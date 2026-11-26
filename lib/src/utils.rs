use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::fs::File;
use std::io;
use std::io::{BufReader, ErrorKind, Read};

pub fn hex_dump(buf: &[u8]) -> String {
    buf.iter()
        .fold(String::with_capacity(2 * buf.len()), |str, b| {
            str + format!("{:02x}", b).as_str()
        })
}

pub fn hex_dump_uppercase(buf: &[u8]) -> String {
    buf.iter()
        .fold(String::with_capacity(2 * buf.len()), |str, b| {
            str + format!("{:02X}", b).as_str()
        })
}

/// Can hold either of the options
pub enum Either<L, R> {
    Left(L),
    Right(R),
}

impl<L, R> Either<L, R> {
    pub fn with_left(l: L) -> Self {
        Self::Left(l)
    }

    pub fn with_right(r: R) -> Self {
        Self::Right(r)
    }

    /// Apply the function to the object in case it contains the [`Self::Left`] option.
    /// Otherwise, do nothing.
    pub fn map_left<F, T>(self, f: F) -> Either<T, R>
    where
        F: FnOnce(L) -> T,
    {
        match self {
            Self::Left(x) => Either::<T, R>::Left(f(x)),
            Self::Right(x) => Either::<T, R>::Right(x),
        }
    }

    /// Apply the function to the object in case it contains the [`Self::Right`] option.
    /// Otherwise, do nothing.
    pub fn map_right<F, T>(self, f: F) -> Either<L, T>
    where
        F: FnOnce(R) -> T,
    {
        match self {
            Self::Left(x) => Either::<L, T>::Left(x),
            Self::Right(x) => Either::<L, T>::Right(f(x)),
        }
    }

    /// Apply the functions to the object accordingly to a contained option.
    pub fn map<FL, FR, T>(self, left: FL, right: FR) -> T
    where
        FL: FnOnce(L) -> T,
        FR: FnOnce(R) -> T,
    {
        match self {
            Self::Left(x) => left(x),
            Self::Right(x) => right(x),
        }
    }
}

pub fn load_certs(filename: &str) -> io::Result<Vec<CertificateDer<'static>>> {
    let mut reader = BufReader::new(File::open(filename)?);
    let mut pem_data = String::new();
    reader.read_to_string(&mut pem_data).map_err(|e| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("Failed to read file: {}", e),
        )
    })?;

    CertificateDer::pem_slice_iter(pem_data.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| io::Error::new(ErrorKind::InvalidInput, format!("Invalid cert: {}", e)))
        .map(|certs| certs.into_iter().map(|c| c.into_owned()).collect())
}

pub fn load_private_key(filename: &str) -> io::Result<PrivateKeyDer<'static>> {
    let mut reader = BufReader::new(File::open(filename)?);
    let mut pem_data = String::new();
    reader.read_to_string(&mut pem_data).map_err(|e| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("Failed to read file: {}", e),
        )
    })?;

    PrivateKeyDer::from_pem_slice(pem_data.as_bytes())
        .map_err(|e| io::Error::new(ErrorKind::InvalidInput, format!("Invalid key: {}", e)))
        .map(|key| key.clone_key())
}

pub trait IterJoin {
    type Output;

    /// Like [`Iterator::fold`] but drops the trailing separator
    fn join(self, sep: impl AsRef<str>) -> Self::Output;
}

impl<I, T> IterJoin for I
where
    I: Iterator<Item = T>,
    T: AsRef<str>,
{
    type Output = String;

    fn join(self, sep: impl AsRef<str>) -> Self::Output {
        let mut ret = self.fold(String::new(), |acc, x| acc + x.as_ref() + sep.as_ref());
        if ret.len() > sep.as_ref().len() {
            ret.replace_range((ret.len() - sep.as_ref().len()).., "");
        }

        ret
    }
}

pub trait ToTomlComment {
    /// Prepend each line of string with "# " turning
    /// the whole string it into TOML comment.
    fn to_toml_comment(&self) -> String;
}

impl ToTomlComment for &str {
    fn to_toml_comment(&self) -> String {
        self.lines().map(|x| format!("# {x}")).join("\n")
    }
}

impl ToTomlComment for String {
    fn to_toml_comment(&self) -> String {
        self.as_str().to_toml_comment()
    }
}

#[cfg(test)]
pub(crate) fn make_test_cert_key_file() -> tempfile::NamedTempFile {
    use std::io::Write;

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(
        b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n\
          -----BEGIN PRIVATE KEY-----\nAA==\n-----END PRIVATE KEY-----\n",
    )
    .unwrap();
    file.flush().unwrap();
    file
}

#[cfg(test)]
mod tests {
    use crate::utils::IterJoin;

    #[test]
    fn iter_join() {
        assert_eq!("a.b.c", ["a", "b", "c"].iter().join("."));
        assert_eq!("a", std::iter::once("a").join("x"));
        assert_eq!("", std::iter::empty::<&str>().join("x"));
    }
}
