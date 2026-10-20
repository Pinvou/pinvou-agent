//! Explicit isolated QA operator workflow. Production app/control routing and
//! all installation permissions remain unavailable and are never constructed.
use pinvou_upgrade_metadata_qa::{Bundle, Error, QaStore, read_bounded};
use std::path::Path;
fn emit(value: &impl serde::Serialize) -> Result<(), Error> {
    use std::io::Write;
    let out = std::io::stdout();
    let mut out = out.lock();
    // A private original-byte frame can contain the entire metadata envelope
    // as an escaped string; it is not a larger protocol document allowance.
    let bytes = serde_json::to_vec(value).map_err(|_| Error::Unavailable)?;
    if bytes.len() > 2 * 1_048_576 + 4096 {
        return Err(Error::Unavailable);
    }
    out.write_all(&bytes)
        .and_then(|_| out.write_all(b"\n"))
        .and_then(|_| out.flush())
        .map_err(|_| Error::Unavailable)
}
fn execute(args: &[String]) -> Result<(), Error> {
    match args.first().map(String::as_str) {
        Some("init") if args.len() == 5 => {
            let mut store = QaStore::create_new(
                Path::new(&args[1]),
                &read_bounded(Path::new(&args[2]))?,
                &args[3],
                args[4].parse().map_err(|_| Error::InvalidInput)?,
            )?;
            // create_new has committed. Any subsequent inspection/output
            // failure is uncertain initialization from the caller's viewpoint.
            let result = store.inspect().map_err(|_| Error::Uncertain)?;
            emit(&result).map_err(|_| Error::Uncertain)
        }
        Some("accept") if args.len() == 4 => {
            let file = Path::new(&args[3]);
            let bundle = Bundle::parse(&read_bounded(file)?)?;
            let mut store = QaStore::open_existing(Path::new(&args[1]), &args[2])?;
            let accepted = store.accept(file.parent().ok_or(Error::InvalidInput)?, &bundle)?;
            // The durable transaction has succeeded even if the actual stdout
            // pipe breaks. Do not relabel a lost response as a precommit refusal.
            emit(&accepted).map_err(|_| Error::Uncertain)
        }
        Some("inspect") if args.len() == 3 => {
            emit(&QaStore::open_existing(Path::new(&args[1]), &args[2])?.inspect()?)
        }
        Some("original-after") if args.len() == 5 => emit(
            &QaStore::open_existing(Path::new(&args[1]), &args[2])?
                .original_after(&args[3], args[4].parse().map_err(|_| Error::InvalidInput)?)?,
        ),
        Some("original") if args.len() == 5 => emit(
            &QaStore::open_existing(Path::new(&args[1]), &args[2])?
                .original(&args[3], args[4].parse().map_err(|_| Error::InvalidInput)?)?,
        ),
        Some("production") => Err(Error::Unsupported),
        _ => Err(Error::InvalidInput),
    }
}
fn main() {
    if let Err(error) = execute(&std::env::args().skip(1).collect::<Vec<_>>()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
