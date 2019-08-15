use snafu::Snafu;
use std::borrow::Cow;
use std::io;
use std::result::Result as StdResult;

#[derive(Debug, Snafu)]
pub enum Error<'a> {
    #[snafu(display("Could not parse BinEl `{}`", name))]
    InvalidBinEl {
        name: Cow<'static, str>,
        received_name: Option<String>,
    },
    #[snafu(display("Error writing file: {}", source))]
    Write { source: io::Error },
    #[snafu(display("Error parsing file: {:?}", source))]
    Parse {
        #[snafu(source(false))]
        source: (&'a [u8], nom::error::ErrorKind),
    },
    #[snafu(display("Incomplete data when parsing file"))]
    Incomplete,
}

impl Error<'_> {
    pub fn from_name(name: impl Into<Cow<'static, str>>) -> Self {
        Error::InvalidBinEl {
            name: name.into(),
            received_name: None,
        }
    }

    pub fn wrong_name(name: impl Into<Cow<'static, str>>, received_name: String) -> Self {
        Error::InvalidBinEl {
            name: name.into(),
            received_name: Some(received_name),
        }
    }

    pub fn io(kind: io::ErrorKind, text: String) -> Self {
        Error::Write {
            source: io::Error::new(kind, text),
        }
    }
}

impl From<io::Error> for Error<'_> {
    fn from(source: io::Error) -> Self {
        Error::Write { source }
    }
}

impl<'a> From<nom::Err<Error<'a>>> for Error<'a> {
    fn from(source: nom::Err<Error<'a>>) -> Self {
        use nom::Err;
        match source {
            Err::Incomplete(_) => Error::Incomplete,
            Err::Error(source) => source,
            Err::Failure(source) => source,
        }
    }
}

impl<'a> nom::error::ParseError<&'a [u8]> for Error<'a> {
    fn from_error_kind(input: &'a [u8], kind: nom::error::ErrorKind) -> Self {
        Error::Parse {
            source: (input, kind),
        }
    }

    fn append(input: &'a [u8], kind: nom::error::ErrorKind, _other: Self) -> Self {
        Self::from_error_kind(input, kind)
    }
}

pub type Result<'a, T> = StdResult<T, Error<'a>>;
