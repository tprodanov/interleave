use std::{
    io::{self, Write, BufRead, BufReader, BufWriter},
    fs::File,
    ffi::OsStr,
    path::Path,
};
use flate2::{
    bufread::MultiGzDecoder,
    write::GzEncoder,
};

const BUFFER_SIZE_1MB: usize = 1 << 20;

type GzFile = BufWriter<GzEncoder<File>>;

/// Creates a gzip compressed file.
fn create_gzip(filename: &Path) -> io::Result<GzFile> {
    let file = File::create(filename)?;
    Ok(BufWriter::with_capacity(BUFFER_SIZE_1MB, GzEncoder::new(file, flate2::Compression::default())))
}

type ZstdFile = BufWriter<zstd::stream::write::AutoFinishEncoder<'static, File>>;

fn create_zstd(filename: &Path) -> io::Result<ZstdFile> {
    const LEVEL: i32 = 7;
    let file = File::create(filename)?;
    zstd::stream::write::Encoder::new(file, LEVEL)
        .map(|w| w.auto_finish())
        .map(|w| BufWriter::with_capacity(BUFFER_SIZE_1MB, w))
}

fn create(filename: impl AsRef<Path>) -> io::Result<Box<dyn Write + Send>> {
    let filename = filename.as_ref();
    if filename == OsStr::new("-") || filename == OsStr::new("/dev/stdin") {
        Ok(Box::new(BufWriter::new(io::stdout())))
    } else {
        match filename.extension().and_then(OsStr::to_str) {
            Some("gz") => Ok(Box::new(create_gzip(filename)?)),
            Some("zst") => Ok(Box::new(create_zstd(filename)?)),
            _ => Ok(Box::new(BufWriter::with_capacity(BUFFER_SIZE_1MB, File::create(filename)?))),
        }
    }
}

fn open(filename: impl AsRef<Path>) -> io::Result<Box<dyn BufRead + Send>> {
    let filename = filename.as_ref();
    if filename == OsStr::new("-") || filename == OsStr::new("/dev/stdin") {
        Ok(Box::new(BufReader::new(io::stdin())))
    } else {
        let stream = BufReader::with_capacity(BUFFER_SIZE_1MB, File::open(filename)?);
        match filename.extension().and_then(OsStr::to_str) {
            Some("gz") => Ok(Box::new(BufReader::with_capacity(BUFFER_SIZE_1MB, MultiGzDecoder::new(stream)))),
            Some("zst") => Ok(Box::new(BufReader::with_capacity(BUFFER_SIZE_1MB,
                zstd::stream::read::Decoder::new(stream)?))),
            _ => Ok(Box::new(BufReader::with_capacity(BUFFER_SIZE_1MB, stream))),
        }
    }
}

#[inline(always)]
fn read_line(mut f: impl BufRead, buf: &mut Vec<u8>) -> io::Result<usize> {
    f.read_until(b'\n', buf)
}

// fn interleave(
//     start: u64,
//     prefix: &str,
//     filename1: &str,
//     filename2: &str,
//     f_out: &mut impl Write,
// ) -> io::Result<u64>
// {
//     let mut f_in1 = BufReader::new(MultiGzDecoder::new(BufReader::new(File::open(filename1)?)));
//     let mut f_in2 = BufReader::new(MultiGzDecoder::new(BufReader::new(File::open(filename2)?)));

//     let mut name = Vec::with_capacity(256);
//     write!(name, ">{}.", prefix).unwrap();
//     let k = name.len();
//     let mut junk = Vec::with_capacity(4096);
//     let mut seq1 = Vec::with_capacity(1024);
//     let mut seq2 = Vec::with_capacity(1024);
//     for i in start.. {
//         name.truncate(k);
//         write!(name, "{:X}\n", i).unwrap();
//         junk.clear();
//         let n1 = read_line(&mut f_in1, &mut junk)?;
//         let n2 = read_line(&mut f_in2, &mut junk)?;
//         assert_eq!(n1, n2, "Name lines have different lengths");
//         if n1 == 0 {
//             return Ok(i)
//         }

//         seq1.clear();
//         seq2.clear();
//         assert!(read_line(&mut f_in1, &mut seq1)? > 0, "Sequence empty for the first read");
//         assert!(read_line(&mut f_in2, &mut seq2)? > 0, "Sequence empty for the second read");
//         f_out.write_all(&name)?;
//         f_out.write_all(&seq1)?;
//         f_out.write_all(&name)?;
//         f_out.write_all(&seq2)?;

//         read_line(&mut f_in1, &mut junk)?;
//         read_line(&mut f_in1, &mut junk)?;
//         read_line(&mut f_in2, &mut junk)?;
//         read_line(&mut f_in2, &mut junk)?;
//     }
//     unreachable!()
// }

fn interleave(
    start: u64,
    mut f_in1: impl BufRead,
    mut f_in2: impl BufRead,
    f_out: &mut impl Write,
    preserve_name: bool,
) -> io::Result<u64>
{
    let mut name = Vec::with_capacity(64);
    let mut junk = Vec::with_capacity(4096);
    let mut rec1 = Vec::with_capacity(2048);
    let mut rec2 = Vec::with_capacity(2048);
    for i in start.. {
        name.clear();
        junk.clear();
        let n1 = read_line(&mut f_in1, if preserve_name { &mut name } else { &mut junk })?;
        if !preserve_name {
            write!(name, "@R{:X}\n", i).unwrap();
        }
        let n2 = read_line(&mut f_in2, &mut junk)?;
        assert_eq!(n1, n2, "Name lines have different lengths");
        if n1 == 0 {
            return Ok(i)
        }

        rec1.clear();
        rec2.clear();
        assert!(read_line(&mut f_in1, &mut rec1)? > 0, "Sequence empty for the first read");
        read_line(&mut f_in1, &mut rec1)?;
        read_line(&mut f_in1, &mut rec1)?;

        assert!(read_line(&mut f_in2, &mut rec2)? > 0, "Sequence empty for the second read");
        read_line(&mut f_in2, &mut rec2)?;
        read_line(&mut f_in2, &mut rec2)?;

        f_out.write_all(&name)?;
        f_out.write_all(&rec1)?;
        f_out.write_all(&name)?;
        f_out.write_all(&rec2)?;
    }
    unreachable!()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 5,
        "Expected exactly 4 arguments: first and second read files; output file; keep read names? [preserve/forget].");
    // Arguments:
    // 1. gzip file with first reads (multiple files allowed through comma)
    // 2. gzip file with second reads (-//-)
    // 3. gzip output fastq file.
    // 4. preserve/forget: should we preserve read names or not.

    let preserve_arg = &args[4];
    assert!(preserve_arg == "preserve" || preserve_arg == "forget");
    let preserve = preserve_arg == "preserve";
    let filenames1: Vec<_> = args[1].split(',').collect();
    let filenames2: Vec<_> = args[2].split(',').collect();
    assert_eq!(filenames1.len(), filenames2.len(), "Unequal number of first and second read files");

    let out_filename = &args[3];
    let tmp_filename = format!("{}.tmp", out_filename);
    let mut f_out = create(&tmp_filename).unwrap();
    let mut start = 1;
    for (fname1, fname2) in filenames1.iter().zip(&filenames2) {
        let f1 = open(fname1).unwrap();
        let f2 = open(fname2).unwrap();
        start = interleave(start, f1, f2, &mut f_out, preserve).unwrap();
    }
    std::fs::rename(&tmp_filename, &out_filename).unwrap();
}
