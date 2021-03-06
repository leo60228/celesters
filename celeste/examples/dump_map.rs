use celeste::{
    binel::{serialize::*, *},
    *,
};

fn main() {
    let map_bytes = include_bytes!("empty.bin");
    let map_bin = parser::take_file::<Error>(map_bytes).unwrap().1;
    println!("{:#?}", map_bin); // pretty print
    let map_data = match maps::Map::from_binel(BinElValue::Element(map_bin.root)) {
        Ok(map) => map,
        Err(err) => {
            println!("{}", err);
            return;
        }
    };
    println!("{:#?}", map_data);
}
