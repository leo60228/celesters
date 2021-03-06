#![recursion_limit = "1024"]

use async_std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use async_std::task;
use broadcaster::BroadcastChannel;
use celeste::ghostnet::*;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::lock::Mutex;
use futures::prelude::*;
use futures_intrusive::channel::{
    shared::{state_broadcast_channel, StateReceiver, StateSender},
    StateId,
};
use slice_deque::SliceDeque;
use smallvec::*;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::convert::TryFrom;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

type Result<'a, T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'a>>; // 4

type MPlayerId = ([u8; 4], MPlayer<'static>);
type MPlayerMap = BTreeMap<u32, MPlayerId>;
type MPlayerLock = Arc<Mutex<MPlayerMap>>;

type UdpPacket = (SocketAddr, Vec<u8>);
type UdpChannel = UnboundedSender<UdpPacket>;
type UdpChannelMap = HashMap<IpAddr, UdpChannel>;
type UdpMapLock = Arc<Mutex<UdpChannelMap>>;

pub async fn server(addr: impl ToSocketAddrs + Clone) -> Result<'static, ()> {
    let tcp_broadcast = BroadcastChannel::<Vec<u8>>::new();
    let (udp_broadcast_tx, udp_broadcast_rx) = state_broadcast_channel::<Vec<u8>>();

    let listener = TcpListener::bind(addr.clone()).await?;
    let mut incoming = listener.incoming().fuse();

    let udp = Arc::new(UdpSocket::bind(addr).await?);

    let udp_map: UdpMapLock = Default::default();
    let mplayers: MPlayerLock = Default::default();

    let _udp_handle = task::spawn({
        let udp_map = udp_map.clone();
        let udp = udp.clone();
        async move {
            let mut data = [0u8; 1024];
            while let Ok((read, addr)) = udp.recv_from(&mut data).await {
                let channel = udp_map.lock().await.get(&addr.ip()).map(Clone::clone);
                if let Some(channel) = channel {
                    let _ = channel.unbounded_send((addr, Vec::from(&data[..read])));
                }
            }
        }
    });

    let mut id = 1;
    let chat_id = Arc::new(AtomicU32::new(1));

    let mut mplayer_broadcast = tcp_broadcast.clone();

    loop {
        futures::select! {
            sock = incoming.next() => {
                println!("{:?}", sock);
                match sock {
                    None => continue,
                    Some(Err(err)) => eprintln!("error connecting to socket: {}", err),
                    Some(Ok(sock)) => {
                        println!("getting addr");
                        let addr = match sock.peer_addr() {
                            Ok(addr) => addr,
                            Err(_) => continue,
                        };
                        let (tx, rx) = mpsc::unbounded();
                        println!("inserting to udp_map");
                        udp_map.lock().await.insert(addr.ip(), tx);
                        println!("done");
                        let tcp_broadcast = tcp_broadcast.clone();
                        let mplayers = mplayers.clone();
                        let _handle = task::spawn(handle(
                            sock,
                            udp.clone(),
                            rx,
                            tcp_broadcast,
                            (udp_broadcast_rx.clone(), udp_broadcast_tx.clone()),
                            id,
                            chat_id.clone(),
                            mplayers,
                        ));
                        id += 1;
                        println!("done");
                    }
                }
            },
            f = mplayer_broadcast.recv().fuse() => {
                let buf: Vec<u8> = if let Some(f) = f { f } else { continue };
                if let Ok((_, frame)) = frame::<()>(&buf) {
                    let raw_chunks = frame.raw_chunks;
                    let head = raw_chunks.iter().find(|c| c.typ == ChunkType::HHead);
                    let player = raw_chunks.iter().find(|c| c.typ == ChunkType::MPlayer);
                    if let (Some(head), Some(player)) = (head, player) {
                        if let (Ok(ChunkData::HHead(head)), Ok(ChunkData::MPlayer(player))) = (ChunkData::try_from(head), ChunkData::try_from(player)) {
                            if player.name == "" {
                                mplayers.lock().await.remove(&head.id);
                            } else {
                                let bytes = head.id.to_le_bytes();
                                mplayers.lock().await.insert(head.id, (bytes, player.into_owned()));
                            }
                        }
                    }
                };
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn handle(
    sock: TcpStream,
    udp: Arc<UdpSocket>,
    mut udp_recv: impl Stream<Item = (SocketAddr, Vec<u8>)> + Send + Sync + Unpin + 'static,
    mut tcp_broadcast_rx: BroadcastChannel<Vec<u8>>,
    (udp_broadcast_rx, udp_broadcast_tx): (StateReceiver<Vec<u8>>, StateSender<Vec<u8>>),
    id: u32,
    chat_id: Arc<AtomicU32>,
    players: MPlayerLock,
) {
    println!("mpsc");
    let (_response_tx, mut response_rx) = mpsc::unbounded::<Vec<u8>>();
    println!("clone");
    let tcp_broadcast_tx = tcp_broadcast_rx.clone();
    println!("split");
    let (mut read, mut write) = (&sock, &sock);

    println!("mutex");
    let udp_addr = Arc::new(Mutex::new(None));

    let send = async move {
        let head = ChunkData::HHead(HHead { id });
        let info = ChunkData::MServerInfo(MServerInfo { name: "ghastly" });
        let req = ChunkData::MRequest(MRequest {
            id: ChunkType::MPlayer,
        });
        let frame = Frame {
            raw_chunks: smallvec![head.into(), info.into(), req.into(),],
        };

        frame.write(&mut write).await?;

        let head = ChunkData::HHead(HHead { id: 0 });
        let server = ChunkData::MPlayer(MPlayer {
            name: "server".into(),
            ..Default::default()
        });
        let frame = Frame {
            raw_chunks: smallvec![server.into(), head.into(),],
        };
        frame.write(&mut write).await?;

        {
            let players = players.lock().await;

            let player_frames: Vec<Frame> = players
                .values()
                .map(|(id, player)| {
                    let head = Chunk {
                        typ: ChunkType::HHead,
                        data: Cow::Borrowed(id),
                    };
                    Frame {
                        raw_chunks: smallvec![ChunkData::MPlayer(player.clone()).into(), head,],
                    }
                })
                .collect();

            for frame in player_frames {
                frame.write(&mut write).await?;
            }
        }

        loop {
            futures::select! {
                response = response_rx.next() => {
                    let response = response.ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "couldn't get next response",
                        )
                    })?;
                    write.write_all(&response).await?;
                },
                broadcast = tcp_broadcast_rx.recv().fuse() => {
                    let broadcast = broadcast.unwrap();
                    println!("got broadcast: {:?}...", &broadcast[..16]);
                    write.write_all(&broadcast).await?;
                },
                complete => {
                    return Result::Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "couldn't get next response").into())
                }
            }
        }

        #[allow(unreachable_code)]
        Result::Ok(())
    };

    let recv = async move {
        let mut buf: SliceDeque<u8> = SliceDeque::new();
        let mut start = 0;
        let mut welcomed = false;
        let mut player_name = None;

        loop {
            let unparsed;

            match frame::<celeste::Error>(&buf[..start]) {
                Ok((rem, frame)) => {
                    unparsed = rem.len();
                    eprintln!("got frame");
                    println!("{:#?}", frame);
                    for chunk in frame.raw_chunks {
                        match chunk.typ {
                            ChunkType::MChat => {
                                if let Ok(ChunkData::MChat(chat)) = ChunkData::try_from(&chunk) {
                                    println!("got mchat");
                                    let chat = ChunkData::MChat(MChat {
                                        red: 255,
                                        blue: 255,
                                        green: 255,
                                        text: chat.text,
                                        id: chat_id.fetch_add(1, Ordering::SeqCst),
                                        ..Default::default()
                                    });
                                    let id = id.to_le_bytes();
                                    let mut data = Vec::new();
                                    let fwd = Frame {
                                        raw_chunks: smallvec![
                                            chat.into(),
                                            Chunk {
                                                typ: ChunkType::HHead,
                                                data: Cow::Borrowed(&id),
                                            },
                                        ],
                                    };
                                    fwd.write(&mut data).await?;
                                    println!("forwarding mchat");
                                    tcp_broadcast_tx.send(&data).await.unwrap();
                                    println!("forwarded mchat");
                                }
                            }
                            ChunkType::MPlayer => {
                                if let Ok(ChunkData::MPlayer(mut chunk)) =
                                    ChunkData::try_from(&chunk)
                                {
                                    println!("got mplayer");
                                    chunk.echo = true;
                                    let cdata = ChunkData::MPlayer(chunk.clone()).into();
                                    let id = id.to_le_bytes();
                                    let mut data = Vec::new();
                                    let fwd = Frame {
                                        raw_chunks: smallvec![
                                            cdata,
                                            Chunk {
                                                typ: ChunkType::HHead,
                                                data: Cow::Borrowed(&id),
                                            },
                                        ],
                                    };
                                    fwd.write(&mut data).await?;
                                    println!("forwarding mplayer");
                                    tcp_broadcast_tx.send(&data).await.unwrap();
                                    println!("forwarded mplayer");
                                    if !welcomed {
                                        welcomed = true;
                                        player_name = Some(chunk.name.to_string());
                                        let welcome = format!("Welcome, {}! This is alpha-quality software. Please report issues at https://discord.gg/TkzxByV.", chunk.name);
                                        println!("{}", welcome);
                                        let message = ChunkData::MChat(MChat {
                                            text: &welcome,
                                            red: 255,
                                            blue: 0,
                                            green: 255,
                                            id: chat_id.fetch_add(1, Ordering::SeqCst),
                                            ..Default::default()
                                        });
                                        let frame = Frame {
                                            raw_chunks: smallvec![
                                                ChunkData::HHead(HHead { id: 0 }).into(),
                                                message.into()
                                            ],
                                        };
                                        println!("{:?}", frame);
                                        data.clear();
                                        frame.write(&mut data).await?;
                                        tcp_broadcast_tx.send(&data).await.unwrap();
                                        println!("sent welcome");
                                    }
                                }
                            }
                            ChunkType::Unknown(ty) => {
                                println!("unknown chunk {:?}", ty);
                            }
                            _ => continue,
                        }
                    }
                }
                res @ Err(nom::Err::Incomplete(_)) => {
                    std::mem::drop(res);

                    buf.extend(std::iter::repeat(0).take(start + 128 - buf.len()));
                    eprintln!("reading");
                    let read = read.read(&mut buf[start..]).await?;
                    if read != 0 {
                        start += read;
                    } else {
                        println!("disconnected");
                        let mut data = Vec::new();
                        let head = ChunkData::HHead(HHead { id });
                        let player = ChunkData::MPlayer(Default::default());
                        let frame = Frame {
                            raw_chunks: smallvec![head.into(), player.into(),],
                        };
                        frame.write(&mut data).await?;
                        println!("forwarding mplayer");
                        tcp_broadcast_tx.send(&data).await.unwrap();
                        println!("forwarded mplayer");

                        if let Some(player_name) = player_name {
                            let goodbye = format!("Cya, {}!", player_name);
                            println!("{}", goodbye);
                            let message = ChunkData::MChat(MChat {
                                text: &goodbye,
                                red: 255,
                                blue: 0,
                                green: 255,
                                id: chat_id.fetch_add(1, Ordering::SeqCst),
                                ..Default::default()
                            });
                            let frame = Frame {
                                raw_chunks: smallvec![
                                    ChunkData::HHead(HHead { id: 0 }).into(),
                                    message.into()
                                ],
                            };
                            println!("{:?}", frame);
                            data.clear();
                            frame.write(&mut data).await?;
                            tcp_broadcast_tx.send(&data).await.unwrap();
                            println!("sent goodbye");
                        }

                        return Result::Err(
                            std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "client disconnected",
                            )
                            .into(),
                        );
                    }
                    continue;
                }
                Err(_) => {
                    return Result::Err(
                        std::io::Error::new(std::io::ErrorKind::InvalidData, "bad chunk").into(),
                    )
                }
            }

            let parsed = start - unparsed;
            start -= parsed;
            for _ in 0..parsed {
                buf.pop_front();
            }
        }

        #[allow(unreachable_code)]
        Result::Ok(())
    };

    let udp_broadcast_addr = udp_addr.clone();
    let udp_broadcaster = async move {
        let mut state = StateId::new();
        while let Some((new_state, frame)) = udp_broadcast_rx.receive(state).await {
            state = new_state;

            let addr;

            match *udp_broadcast_addr.lock().await {
                Some(a) => addr = a,
                None => continue,
            }

            let mut sent = 0;

            while sent < frame.len() {
                let just_sent = udp.send_to(&frame[sent..], addr).await?;
                if just_sent == 0 {
                    *udp_broadcast_addr.lock().await = None;
                    continue;
                }
                sent += just_sent;
            }
        }

        Result::<()>::Err(
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "couldn't get next response",
            )
            .into(),
        )
    };

    let udp_fut = async move {
        let mut buf: SliceDeque<u8> = SliceDeque::new();

        loop {
            let unparsed;

            match frame::<celeste::Error>(&buf) {
                Ok((rem, frame)) => {
                    unparsed = rem.len();
                    for chunk in frame.raw_chunks {
                        if chunk.typ == ChunkType::UUpdate {
                            let mut buf = Vec::new();
                            let head = ChunkData::HHead(HHead { id });
                            let frame = Frame {
                                raw_chunks: smallvec![head.into(), chunk],
                            };
                            frame.write(&mut buf).await?;
                            udp_broadcast_tx.send(buf).unwrap();
                        }
                    }
                }
                res @ Err(_) => {
                    std::mem::drop(res);

                    let (addr, recv) = match udp_recv.next().await {
                        Some(recv) => recv,
                        None => {
                            eprintln!("warning: new client connected before udp stream ready"); // FIXME: reevaluate this
                            return Ok(());
                        }
                    };

                    *udp_addr.lock().await = Some(addr);

                    buf.extend(recv);
                    continue;
                }
            }

            let parsed = buf.len() - unparsed;
            for _ in 0..parsed {
                buf.pop_front();
            }
        }

        #[allow(unreachable_code)]
        Result::Ok(())
    };

    println!("handle done");
    if let Err(err) = future::try_join4(send, recv, udp_fut, udp_broadcaster).await {
        eprintln!("error handling socket: {}", err);
    }
}

fn main() {
    task::block_on(server("0.0.0.0:2782")).unwrap();
}
