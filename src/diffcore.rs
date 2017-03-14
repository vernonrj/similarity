/**
 * Similarity - determine how similar two files are.
 *
 * Based on git's estimate_similarity function in diffcore-rename.c
 */
use std::collections::hash_map::{HashMap, DefaultHasher};
use std::cmp;
use std::fmt;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::path::Path;
use std::vec;

use memchr;

const MAX_SCORE: f64 = 60000.0;
const MIN_SCORE: f64 = 30000.0;

use DiffResult;


/// Estimate the similarity of two files.
pub fn estimate_similarity(left: SpanhashTop, right: SpanhashTop) -> DiffResult<f64> {
    let left_size = left.len();
    let right_size = right.len();
    let max_size = cmp::max(left_size, right_size);
    let base_size = cmp::min(left_size, right_size);
    let delta_size = max_size - base_size;
    // We would not consider edits that change the file size so
    // drastically.  delta_size must be smaller than
    // (MAX_SCORE-minimum_score)/MAX_SCORE * min(src->size, dst->size).
    //
    // Note that base_size == 0 case is handled here already
    // and the final score computation below would not have a
    // divide-by-zero issue.
    //
    if max_size as f64 * (MAX_SCORE - MIN_SCORE) < delta_size as f64 * MAX_SCORE {
        return Ok(0.0);
    }
    // Check file sizes. If there's an empty file, we can stop here.
    match (left_size, right_size) {
        (0, 0) => return Ok(MAX_SCORE),
        (0, _) | (_, 0) => return Ok(0.0),
        (_, _) => (),
    }

    let (copied, _) = count_changes(left, right);
    Ok(copied as f64 * MAX_SCORE / max_size as f64)
}

/// Count the number of changes between two files
fn count_changes(left: SpanhashTop, right: SpanhashTop) -> (usize, usize) {
    let mut source_top = left.into_iter();
    let mut dest_top = right.into_iter();
    let mut d: Spanhash = dest_top.next().unwrap_or_default();
    let mut literal_added = 0;
    let mut source_copied = 0;
    while let Some(s) = source_top.next() {
        while d.occurrences != 0 {
            if d.hashval >= s.hashval {
                break;
            }
            literal_added += d.occurrences;
            d = dest_top.next().unwrap_or_default();
        }
        let src_cnt = s.occurrences;
        let mut dst_cnt = 0;
        if d.occurrences > 0 && d.hashval == s.hashval {
            dst_cnt = d.occurrences;
            d = dest_top.next().unwrap_or_default();
        }
        if src_cnt < dst_cnt {
            literal_added += dst_cnt - src_cnt;
            source_copied += src_cnt;
        } else {
            source_copied += dst_cnt;
        }
    }
    while d.occurrences > 0 {
        literal_added += d.occurrences;
        d = dest_top.next().unwrap_or_default();
    }
    (source_copied, literal_added)
}


/// Hashing of a file
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpanhashTop(HashMap<Vec<u8>, (u64, usize)>);

impl SpanhashTop {
    pub fn from_file<P: AsRef<Path>>(p: P, is_binary: bool) -> io::Result<Self> {
        let f = File::open(p.as_ref())?;
        Self::from_reader(f, is_binary)
    }
    pub fn from_reader<R: Read>(mut reader: R, is_binary: bool) -> io::Result<Self> {
        let max_line_length = 64;
        let mut h = HashMap::new();
        let mut buf: Vec<u8> = vec![0; 128];
        let mut is_done = false;
        let mut buf_len = 0;
        while !is_done {
            buf.resize(max_line_length, 0);
            match reader.read(&mut buf[buf_len..max_line_length]) {
                Ok(0) => {
                    is_done = true;
                }
                Ok(n) => {
                    buf_len += n;
                    if buf_len < max_line_length {
                        continue;
                    }
                }
                Err(_) => {
                    is_done = true;
                    ()
                }
            }
            while buf_len > 0 {
                let end_idx = if let Some(idx) = memchr::memchr(b'\n', &buf[..buf_len]) {
                    idx + 1
                } else if buf_len < max_line_length {
                    break;
                } else {
                    max_line_length
                };
                let rest = buf.split_off(end_idx);
                buf_len = buf_len - end_idx;
                let has_crlf = end_idx > 1 && buf[end_idx - 1] == b'\n' &&
                               buf[end_idx - 2] == b'\r';
                if !is_binary && has_crlf {
                    // Ignore CR in CRLF sequence if text
                    buf[end_idx - 2] = b'\n';
                    buf.pop();
                }
                let hashed = {
                    let mut hasher = DefaultHasher::new();
                    buf.hash(&mut hasher);
                    hasher.finish()
                };
                let cnt = buf.len();
                let mut e = h.entry(buf).or_insert((hashed, 0));
                e.1 += cnt;
                buf = rest;
            }
        }
        Ok(SpanhashTop(h))
    }
    pub fn len(&self) -> usize {
        self.0.values().fold(0, |a, &(_, occ)| a + occ)
    }
}

impl IntoIterator for SpanhashTop {
    type IntoIter = vec::IntoIter<Spanhash>;
    type Item = Spanhash;
    fn into_iter(self) -> Self::IntoIter {
        let mut v: Vec<Self::Item> = self.0
            .into_iter()
            .map(|(data, (hashed, occ))| {
                Spanhash {
                    data: data,
                    hashval: hashed,
                    occurrences: occ,
                }
            })
            .collect();
        v.sort_by(|left, right| {
            // We want to sort occurrence from largest to smallest.
            // Our second sort key will be the hash value, which
            // we'll sort from smallest to largest.
            match (left.occurrences, right.occurrences) {
                (0, 0) => return cmp::Ordering::Equal,
                (0, _) => return cmp::Ordering::Greater,
                (_, 0) => return cmp::Ordering::Less,
                (_, _) => (),
            }
            left.hashval.cmp(&right.hashval)
        });
        v.into_iter()
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Spanhash {
    data: Vec<u8>,
    hashval: u64,
    occurrences: usize,
}

impl fmt::Debug for Spanhash {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f,
               "Spanhash {{ data: {data:?} ({data_str}), hashval: 0x{h:x}, occurrences: {o} }}",
               data = self.data,
               data_str = String::from_utf8_lossy(&self.data).replace("\n", "\\n"),
               h = self.hashval,
               o = self.occurrences)
    }
}
