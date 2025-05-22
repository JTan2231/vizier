use crate::dbio::EMBED_DIM;

pub trait Serialize {
    fn to_bytes(&self) -> Vec<u8>;
    fn from_bytes(bytes: &[u8], cursor: usize) -> Result<(Self, usize), std::io::Error>
    where
        Self: Sized;
}

macro_rules! primitive_serialize {
    ($($t:ty),*) => {
        $(
            impl Serialize for $t {
                fn to_bytes(&self) -> Vec<u8> {
                    self.to_be_bytes().to_vec()
                }

                fn from_bytes(bytes: &[u8], cursor: usize) -> Result<(Self, usize), std::io::Error> {
                    let size = std::mem::size_of::<Self>();
                    let value = Self::from_be_bytes(bytes[cursor..cursor + size].try_into().unwrap());

                    Ok((value, size))
                }
            }
        )*
    }
}

primitive_serialize!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64);

impl Serialize for String {
    fn to_bytes(&self) -> Vec<u8> {
        let self_bytes = self.as_bytes();
        let mut bytes = (self_bytes.len() as u32).to_be_bytes().to_vec();
        bytes.extend(self_bytes);
        bytes
    }

    fn from_bytes(bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
        let (len, count) = u32::from_bytes(bytes, cursor)?;
        cursor += count;

        let len = len as usize;
        let value = String::from_utf8(bytes[cursor..cursor + len].to_vec()).unwrap();

        Ok((value, len + count))
    }
}

impl<T: Serialize> Serialize for Option<T> {
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            Some(value) => {
                let mut bytes = vec![1];
                bytes.extend(value.to_bytes());
                bytes
            }
            None => vec![0],
        }
    }

    fn from_bytes(bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
        let has_value = bytes[cursor] == 1;
        cursor += 1;
        if has_value {
            let (value, size) = T::from_bytes(bytes, cursor)?;
            Ok((Some(value), size + 1))
        } else {
            Ok((None, 1))
        }
    }
}

macro_rules! tuple_serialize_impl {
    ($($i:tt : $t:ident),+) => {
        impl<$($t: Serialize),+> Serialize for ($($t,)+) {
            fn to_bytes(&self) -> Vec<u8> {
                let mut bytes = Vec::new();
                $(
                    bytes.extend(self.$i.to_bytes());
                )+
                bytes
            }

            #[allow(unused_assignments)]
            fn from_bytes(bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
                let mut total_size = 0;
                Ok(((
                    $(
                        {
                            let (value, count) = <$t>::from_bytes(bytes, cursor)?;
                            cursor += count;
                            total_size += count;

                            value
                        },
                    )+
                ), total_size))
            }
        }
    };
}

tuple_serialize_impl!(0: T0);
tuple_serialize_impl!(0: T0, 1: T1);

macro_rules! array_serialize_impl {
    ($t:ty, $len:expr) => {
        impl Serialize for [$t; $len] {
            fn to_bytes(&self) -> Vec<u8> {
                self.iter().flat_map(|value| value.to_bytes()).collect()
            }

            #[allow(unused_assignments)]
            fn from_bytes(
                bytes: &[u8],
                mut cursor: usize,
            ) -> Result<(Self, usize), std::io::Error> {
                let mut total_size = 0;
                let mut array = [<$t>::default(); $len];
                for i in 0..$len {
                    let (value, count) = <$t>::from_bytes(bytes, cursor)?;
                    total_size += count;
                    cursor += count;

                    array[i] = value;
                }

                Ok((array, total_size))
            }
        }
    };
}

array_serialize_impl!(f32, EMBED_DIM);

impl<T: Serialize> Serialize for Vec<T> {
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let len = self.len() as u32;
        bytes.extend(len.to_bytes());
        for value in self.iter() {
            bytes.extend(value.to_bytes());
        }

        bytes
    }

    fn from_bytes(bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
        let (len, count) = u32::from_bytes(bytes, cursor)?;
        cursor += count;

        let len = len as usize;
        let mut values = Vec::with_capacity(len);
        let mut total_size = count;
        for _ in 0..len {
            let (value, count) = T::from_bytes(bytes, cursor)?;
            cursor += count;
            total_size += count;

            values.push(value);
        }

        Ok((values, total_size))
    }
}

impl<A, B> Serialize for std::collections::HashMap<A, B>
where
    A: Serialize + std::hash::Hash + Eq,
    B: Serialize,
{
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let len = self.len() as u32;
        bytes.extend(len.to_bytes());
        for (key, value) in self.iter() {
            bytes.extend(key.to_bytes());
            bytes.extend(value.to_bytes());
        }

        bytes
    }

    fn from_bytes(bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
        let (len, count) = u32::from_bytes(bytes, cursor)?;
        cursor += count;

        let len = len as usize;
        let mut map = std::collections::HashMap::with_capacity(len);
        let mut total_size = count;
        for _ in 0..len {
            let (key, count) = A::from_bytes(bytes, cursor)?;
            cursor += count;
            total_size += count;

            let (value, count) = B::from_bytes(bytes, cursor)?;
            cursor += count;
            total_size += count;

            map.insert(key, value);
        }

        Ok((map, total_size))
    }
}

impl<T: Serialize + std::hash::Hash + std::cmp::Eq> Serialize for std::collections::HashSet<T> {
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let len = self.len() as u32;
        bytes.extend(len.to_bytes());
        for value in self.iter() {
            bytes.extend(value.to_bytes());
        }

        bytes
    }

    fn from_bytes(bytes: &[u8], mut cursor: usize) -> Result<(Self, usize), std::io::Error> {
        let (len, count) = u32::from_bytes(bytes, cursor)?;
        cursor += count;

        let len = len as usize;
        let mut values = std::collections::HashSet::new();
        let mut total_size = count;
        for _ in 0..len {
            let (value, count) = T::from_bytes(bytes, cursor)?;
            cursor += count;
            total_size += count;

            values.insert(value);
        }

        Ok((values, total_size))
    }
}

impl Serialize for std::path::PathBuf {
    fn to_bytes(&self) -> Vec<u8> {
        self.to_string_lossy().to_string().to_bytes()
    }

    fn from_bytes(bytes: &[u8], cursor: usize) -> Result<(Self, usize), std::io::Error> {
        let (string, count) = String::from_bytes(bytes, cursor)?;
        Ok((std::path::PathBuf::from(string), count))
    }
}
