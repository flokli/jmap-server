/*
 * Copyright (c) 2020-2022, Stalwart Labs Ltd.
 *
 * This file is part of the Stalwart JMAP Server.
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of
 * the License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU Affero General Public License for more details.
 * in the LICENSE file at the top-level directory of this distribution.
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * You can be released from the requirements of the AGPLv3 license by
 * purchasing a commercial license. Please contact licensing@stalw.art
 * for more details.
*/

use std::io::Write;

use store::blob::{BlobId, BLOB_HASH_LEN};
use store::serialize::base32::{Base32Reader, Base32Writer};
use store::serialize::leb128::{Leb128Iterator, Leb128Writer};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JMAPBlob {
    pub id: BlobId,
    pub section: Option<BlobSection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlobSection {
    pub offset_start: usize,
    pub size: usize,
    pub encoding: u8,
}

impl JMAPBlob {
    pub fn new(id: BlobId) -> Self {
        JMAPBlob { id, section: None }
    }

    pub fn new_section(id: BlobId, offset_start: usize, offset_end: usize, encoding: u8) -> Self {
        JMAPBlob {
            id,
            section: BlobSection {
                offset_start,
                size: offset_end - offset_start,
                encoding,
            }
            .into(),
        }
    }

    pub fn parse(id: &str) -> Option<Self>
    where
        Self: Sized,
    {
        let (is_local, encoding) = match id.as_bytes().first()? {
            b'b' => (false, None),
            b'a' => (true, None),
            b @ b'c'..=b'g' => (true, Some(*b - b'c')),
            b @ b'h'..=b'l' => (false, Some(*b - b'h')),
            _ => {
                return None;
            }
        };

        let mut it = Base32Reader::new(id.get(1..)?.as_bytes());
        let mut hash = [0; BLOB_HASH_LEN];

        for byte in hash.iter_mut().take(BLOB_HASH_LEN) {
            *byte = it.next()?;
        }

        Some(JMAPBlob {
            id: if is_local {
                BlobId::Local { hash }
            } else {
                BlobId::External { hash }
            },
            section: if let Some(encoding) = encoding {
                BlobSection {
                    offset_start: it.next_leb128()?,
                    size: it.next_leb128()?,
                    encoding,
                }
                .into()
            } else {
                None
            },
        })
    }

    pub fn start_offset(&self) -> usize {
        if let Some(section) = &self.section {
            section.offset_start
        } else {
            0
        }
    }
}

impl From<&BlobId> for JMAPBlob {
    fn from(id: &BlobId) -> Self {
        JMAPBlob::new(id.clone())
    }
}

impl From<BlobId> for JMAPBlob {
    fn from(id: BlobId) -> Self {
        JMAPBlob::new(id)
    }
}

impl Default for JMAPBlob {
    fn default() -> Self {
        Self {
            id: BlobId::Local {
                hash: [0; BLOB_HASH_LEN],
            },
            section: None,
        }
    }
}

impl serde::Serialize for JMAPBlob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}

struct JMAPBlobVisitor;

impl<'de> serde::de::Visitor<'de> for JMAPBlobVisitor {
    type Value = JMAPBlob;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a valid JMAP state")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(JMAPBlob::parse(v).unwrap_or_default())
    }
}

impl<'de> serde::Deserialize<'de> for JMAPBlob {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(JMAPBlobVisitor)
    }
}

impl std::fmt::Display for JMAPBlob {
    #[allow(clippy::unused_io_amount)]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut writer;
        if let Some(section) = &self.section {
            writer =
                Base32Writer::with_capacity(BLOB_HASH_LEN + (std::mem::size_of::<u32>() * 2) + 1);
            writer.push_char(char::from(if self.id.is_local() {
                b'c' + section.encoding
            } else {
                b'h' + section.encoding
            }));
            writer.write(self.id.hash()).unwrap();
            writer.write_leb128(section.offset_start).unwrap();
            writer.write_leb128(section.size).unwrap();
        } else {
            writer = Base32Writer::with_capacity(BLOB_HASH_LEN + 1);
            writer.push_char(if self.id.is_local() { 'a' } else { 'b' });
            writer.write(self.id.hash()).unwrap();
        }

        f.write_str(&writer.finalize())
    }
}
