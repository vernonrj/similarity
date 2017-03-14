#[macro_use]
extern crate clap;
#[macro_use]
extern crate error_chain;
extern crate memchr;

use std::collections::hash_map::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use clap::{App, Arg};

mod errors {
    error_chain!{}
}

pub use errors::Result as DiffResult;
pub use errors::Error as DiffError;
pub use errors::ResultExt;

pub mod diffcore;

pub fn run<P1, P2>(left: P1, right: P2, _is_binary: bool) -> DiffResult<f64>
    where P1: AsRef<Path>,
          P2: AsRef<Path>
{
    let (left, run_len): (HashMap<u32, Vec<(usize, f64)>>, _) =
        trigramize_file_to_table(left.as_ref())?;
    let right: Vec<HashSet<u32>> = trigramize_file(right.as_ref())?;
    let mut sim: Vec<HashMap<usize, f64>> = Vec::new();
    for trigs in right {
        let mut matches: HashMap<usize, f64> = HashMap::new();
        for trigram in trigs {
            let found_lines = if let Some(v) = left.get(&trigram) {
                v
            } else {
                continue;
            };
            for &(line, perc) in found_lines {
                let mut found = matches.entry(line).or_insert(0.0);
                *found += perc;
            }
        }
        let matches: HashMap<_, _> = matches.into_iter()
            .filter(|&(_, v)| v > 0.40)
            .collect();
        sim.push(matches);
    }
    let runs = find_runs(sim);
    let similarity = runs_to_percent(runs, run_len);
    Ok(similarity)
}

fn runs_to_percent(runs: Vec<((usize, usize), (usize, usize), Vec<f64>)>, len: usize) -> f64 {
    if len == 0 {
        // TODO: what to return here?
        return 100.0;
    }
    let mut lines = vec![0.0; len+1];
    for run in runs {
        // println!("{:?}", run);
        let ((start, end), _, percs) = run;
        debug_assert!((start..end + 1).count() == percs.len());
        for (line, perc) in (start..end + 1).zip(percs) {
            lines[line] = f64::max(perc, lines[line]);
        }
    }
    // println!("lines = {:?}", lines);
    lines.iter().skip(1).sum::<f64>() / (lines.len() - 1) as f64 * 100.0
}

fn find_runs(sim: Vec<HashMap<usize, f64>>) -> Vec<((usize, usize), (usize, usize), Vec<f64>)> {
    let mut runs = HashMap::new();
    let mut found_runs: HashMap<usize, ((usize, usize), (usize, usize), Vec<f64>)> = HashMap::new();
    for (idx, right_line) in sim.into_iter().enumerate().map(|(x, y)| (x + 1, y)) {
        let mut expected_runs = HashMap::new();
        for (line, perc) in right_line.into_iter() {
            let (mut left, mut right, mut percs) = runs.remove(&(line - 1))
                .unwrap_or(((line, line), (idx, idx), vec![]));
            left.1 = line;
            right.1 = idx;
            percs.push(perc);
            expected_runs.insert(line, (left, right, percs));
        }
        let runs_keys: HashSet<usize> = runs.keys().cloned().collect();
        let expected_keys: HashSet<usize> = expected_runs.keys().map(|k| k - 1).collect();
        let done_runs = runs_keys.difference(&expected_keys);
        for done in done_runs {
            let run: ((usize, usize), (usize, usize), Vec<f64>) = runs.remove(done).unwrap();
            let length = (run.0).1 - (run.0).0;
            if length > 3 {
                found_runs.insert(done.clone(), run);
            }
        }
        runs = expected_runs;
    }
    for (key, val) in runs {
        let length = (val.0).1 - (val.0).0;
        if length > 3 {
            found_runs.insert(key, val);
        }
    }
    found_runs.into_iter().map(|(_, v)| v).collect()
}

fn trigramize_file_to_table<P>(filename: P) -> DiffResult<(HashMap<u32, Vec<(usize, f64)>>, usize)>
    where P: AsRef<Path>
{
    let f = File::open(filename.as_ref())
        .chain_err(|| format!("failed to open file {}", filename.as_ref().display()))?;
    let lines = BufReader::new(f).lines();
    let mut h = HashMap::new();
    let mut last_index = 0;
    for (idx, line) in lines.enumerate().map(|(x, y)| (x + 1, y)) {
        last_index = idx;
        let line = format!("{}\n", line.chain_err(|| "failed to get line from reader")?);
        let trigram_line = make_trigrams(&line);
        let tri_len = trigram_line.len();
        for tri in trigram_line {
            h.entry(tri)
                .or_insert(Vec::new())
                .push((idx, 1.0 / tri_len as f64));
        }
    }
    Ok((h, last_index))
}

fn trigramize_file<P>(filename: P) -> DiffResult<Vec<HashSet<u32>>>
    where P: AsRef<Path>
{
    let f = File::open(filename.as_ref())
        .chain_err(|| format!("failed to open file {}", filename.as_ref().display()))?;
    let lines = BufReader::new(f).lines();
    Ok(lines.into_iter()
        .map(|l| format!("{}\n", l.unwrap()))
        .map(|t| make_trigrams(&t))
        .collect())
}

fn slice_to_trigram(slice: &[u8]) -> DiffResult<u32> {
    match slice.len() {
        0 => Ok(0),
        1 => Ok((slice[0] as u32) << 16),
        2 => Ok(((slice[0] as u32) << 16) | ((slice[1] as u32) << 8)),
        3 => Ok(((slice[0] as u32) << 16) | ((slice[1] as u32) << 8) | (slice[2] as u32)),
        e => Err(format!("too many elements in slice. expected 3, got {}", e).into()),
    }
}

fn make_trigrams(text: &str) -> HashSet<u32> {
    let bytes = text.as_bytes();
    let mut s: HashSet<u32> = bytes.windows(3).map(|t| slice_to_trigram(t).unwrap()).collect();
    if bytes.len() < 3 {
        // println!("> len = {}", bytes.len());
        if bytes.len() > 0 {
            let len = bytes.len();
            s.insert(slice_to_trigram(&bytes[len - 1..]).unwrap());
        }
        if bytes.len() > 1 {
            let len = bytes.len();
            s.insert(slice_to_trigram(&bytes[len - 2..]).unwrap());
        }
        // println!(">> s = {:?}", s);
    }
    s
}


pub fn main() {
    let matches = App::new("similarity")
        .version(crate_version!())
        .author("Vernon Jones <vernonrjones@gmail.com>")
        .about("prints how similar (from 0 to 100%) two files are")
        .arg(Arg::with_name("left")
            .takes_value(true)
            .help("left file to check"))
        .arg(Arg::with_name("right")
            .takes_value(true)
            .help("right file to check"))
        .arg(Arg::with_name("binary")
            .long("binary")
            .help("treat files as binary files (don't ignore CRLF)"))
        .get_matches();
    let left = matches.value_of("left").unwrap();
    let right = matches.value_of("right").unwrap();
    let is_binary = matches.is_present("binary");
    let similarity = match run(left, right, is_binary) {
        Ok(s) => s,
        Err(e) => {
            use std::io::Write;
            let mut stderr = std::io::stderr();
            let stderrfail = "failed to write to stderr";
            writeln!(&mut stderr, "error: {}", e).expect(stderrfail);
            for e in e.iter().skip(1) {
                writeln!(&mut stderr, "caused by: {}", e).expect(stderrfail);
            }
            if let Some(bt) = e.backtrace() {
                writeln!(&mut stderr, "backtrace: {:?}", bt).expect(stderrfail);
            }
            std::process::exit(1);
        }
    };
    println!("{:.2}", similarity);
    // println!("{:.2}", similarity as f64 * 100.0 / MAX_SCORE);
}
