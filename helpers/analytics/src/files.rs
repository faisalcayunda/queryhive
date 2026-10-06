//! Local files as tables: `register_file(name, path, format)` (blueprint section 14.11).
//!
//! A file is read, never copied into a store and never charged to the budget until a query
//! reads it; the batches in transit sit inside the query's lease. The helper cannot write
//! anywhere but its spill directory (the kernel sandbox sees to that), so a file is only
//! ever read.
//!
//! * `path` is one file, or one directory of files of the same kind. The path is turned
//!   into a URL directly, not parsed, so a file called `data[1].csv` is that file and not
//!   a glob.
//! * CSV that is compressed (`.csv.gz` and the rest) is refused: the compression features
//!   of DataFusion are off (section 14.1), and reading gzip as CSV would produce garbage
//!   rather than an error. Compressed Parquet is fine; its codecs come with the format.

use std::path::Path;
use std::sync::Arc;

use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::prelude::SessionContext;
use qh_analytics_proto::FileFormat;

use crate::session::Failure;

const COMPRESSED: [&str; 5] = ["gz", "bz2", "xz", "zst", "zip"];

/// Build the table for a file or directory and infer its schema.
pub async fn open(
    ctx: &SessionContext,
    path: &str,
    format: &FileFormat,
) -> Result<Arc<ListingTable>, Failure> {
    let path = Path::new(path);
    let meta = std::fs::metadata(path)
        .map_err(|error| Failure::invalid(format!("{}: {error}", path.display())))?;
    let url = if meta.is_dir() {
        url::Url::from_directory_path(path)
    } else if meta.is_file() {
        url::Url::from_file_path(path)
    } else {
        return Err(Failure::invalid(format!(
            "{} is not a file or a directory",
            path.display()
        )));
    }
    .map_err(|()| Failure::invalid(format!("{} is not an absolute path", path.display())))?;
    let url = ListingTableUrl::try_new(url, None)?;

    let (file_format, extension): (
        Arc<dyn datafusion::datasource::file_format::FileFormat>,
        &str,
    ) = match format {
        FileFormat::Csv {
            has_header,
            delimiter,
            quote,
        } => {
            let compressed = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| COMPRESSED.contains(&ext.to_ascii_lowercase().as_str()));
            if compressed {
                return Err(Failure::invalid(
                    "Compressed CSV files are not supported. Unpack the file first.",
                ));
            }
            (
                Arc::new(
                    CsvFormat::default()
                        .with_has_header(*has_header)
                        .with_delimiter(*delimiter)
                        .with_quote(*quote),
                ),
                ".csv",
            )
        }
        FileFormat::Parquet => (Arc::new(ParquetFormat::default()), ".parquet"),
    };
    // A file is read whatever it is called (`data.tsv`, `export.txt`); the extension only
    // selects files when a directory is listed.
    let options = ListingOptions::new(file_format).with_file_extension(if meta.is_file() {
        ""
    } else {
        extension
    });
    let schema = options.infer_schema(&ctx.state(), &url).await?;
    let config = ListingTableConfig::new(url)
        .with_listing_options(options)
        .with_schema(schema);
    Ok(Arc::new(ListingTable::try_new(config)?))
}
