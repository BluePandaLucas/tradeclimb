//! Import explicit one-minute EUR CSVs; no synthetic gap filling or venue substitution.
use super::*;
use std::io::{BufRead, BufReader};

pub fn import(
    root: &Path,
    directory: &Path,
    start: i64,
    end: i64,
    accept_gaps: bool,
) -> Result<Manifest> {
    ensure!(
        start >= 0
            && end > start
            && start % MINUTE == 0
            && end % MINUTE == 0
            && end <= timestamp("2026-08-01")?,
        "Kraken research import must precede the reserved final test"
    );
    let mut rows = vec![];
    let mut sources = vec![];
    for market in MARKETS {
        let name = format!("{market}_1.csv");
        let mut reader = BufReader::new(
            File::open(directory.join(&name))
                .with_context(|| format!("missing one-minute Kraken file {name}"))?,
        );
        let mut hash = Sha256::new();
        let mut previous = None;
        let mut count = 0;
        loop {
            let mut line = String::new();
            let length = std::io::Read::by_ref(&mut reader)
                .take(4097)
                .read_line(&mut line)?;
            if length == 0 {
                break;
            }
            ensure!(length <= 4096, "Kraken CSV row exceeds size limit");
            hash.update(line.as_bytes());
            let time = line
                .split(',')
                .next()
                .context("missing timestamp")?
                .trim()
                .parse::<i64>()?
                .checked_mul(1000)
                .context("timestamp overflow")?;
            ensure!(
                time >= 0 && time % MINUTE == 0,
                "one-minute aligned Kraken seconds timestamps required"
            );
            ensure!(
                previous.is_none_or(|p| p < time),
                "duplicate/unordered Kraken CSV timestamp"
            );
            previous = Some(time);
            // Exclude reserved observations before parsing any financial fields.
            if time < start || time >= end {
                continue;
            }
            rows.push(parse_row(market, time, &line)?);
            count += 1;
        }
        ensure!(count > 0, "no requested Kraken candles for {market}");
        sources.push(Source {
            file: name,
            sha256: format!("{:x}", hash.finalize()),
        });
    }
    let audit = directory.join("origin.json");
    let audit_bytes = audit.exists().then(|| fs::read(&audit)).transpose()?;
    if let Some(bytes) = &audit_bytes {
        sources.push(Source {
            file: "origin.json".into(),
            sha256: digest(bytes),
        });
    }
    let manifest = snapshot_from(
        root,
        rows,
        start,
        end,
        accept_gaps,
        Origin {
            venue: "kraken".into(),
            synthetic: false,
            sources,
        },
    )?;
    if let Some(bytes) = audit_bytes {
        atomic_write(
            &root.join("datasets").join(&manifest.id).join("origin.json"),
            &bytes,
        )?;
    }
    Ok(manifest)
}
fn parse_row(market: &str, time: i64, line: &str) -> Result<Candle> {
    let fields: Vec<_> = line.trim().split(',').collect();
    ensure!(
        fields.len() == 7,
        "expected Kraken timestamp,open,high,low,close,volume,trades"
    );
    ensure!(
        fields[6].parse::<u64>()? > 0,
        "Kraken OHLCVT needs a positive trade count"
    );
    let c = Candle {
        market: market.into(),
        time,
        open: fields[1].parse()?,
        high: fields[2].parse()?,
        low: fields[3].parse()?,
        close: fields[4].parse()?,
        volume: fields[5].parse()?,
    };
    c.validate()?;
    ensure!(
        c.volume > Decimal::ZERO,
        "Kraken traded interval needs positive volume"
    );
    Ok(c)
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn venue_and_sources_change_identity_and_gaps_remain_explicit() {
        let root = tempfile::tempdir().unwrap();
        let csv = root.path().join("csv");
        fs::create_dir(&csv).unwrap();
        for market in MARKETS {
            fs::write(
                csv.join(format!("{market}_1.csv")),
                "0,100,101,99,100,1,2\n60,100,101,99,100,1,3\n",
            )
            .unwrap();
        }
        let kraken = import(root.path(), &csv, 0, 120 * MINUTE, false).unwrap();
        assert_eq!(kraken.venue, "kraken");
        assert!(!kraken.accepted);
        assert_eq!(kraken.coverage[0].missing_minutes, 118);
        assert_eq!(kraken.sources.len(), 5);
        let (_, rows) = load(root.path(), &kraken.id).unwrap();
        let native = snapshot(root.path(), rows, 0, 120 * MINUTE, false, false).unwrap();
        assert_ne!(native.id, kraken.id);
        let accepted = import(root.path(), &csv, 0, 120 * MINUTE, true).unwrap();
        assert!(accepted.accepted);
        assert_ne!(accepted.id, kraken.id);
        fs::write(
            csv.join("BTC-EUR_1.csv"),
            "0,100,101,99,100,1,2\n0,100,101,99,100,1,3\n",
        )
        .unwrap();
        assert!(import(root.path(), &csv, 0, 120 * MINUTE, false).is_err());
    }
    #[test]
    fn schema_alignment_and_reserved_dates_are_rejected() {
        assert!(parse_row("BTC-EUR", 0, "0,100,101,99,100,1,0").is_err());
        assert!(parse_row("BTC-EUR", 0, "0,100,99,101,100,1,1").is_err());
        let root = tempfile::tempdir().unwrap();
        assert!(
            import(
                root.path(),
                root.path(),
                timestamp("2026-08-01").unwrap(),
                timestamp("2026-11-01").unwrap(),
                true
            )
            .is_err()
        );
    }
}
