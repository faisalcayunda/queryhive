//! `qh_natural(text) -> binary`: the grid's natural sort key as a SQL function
//! (blueprint section 14.10).
//!
//! `ORDER BY qh_natural(name)` gives the order the grid gives. There is one function and
//! it is `qh_result_store::natural_key`, the same code `set_view` uses, so SQL and the grid
//! cannot grow two natural collations that disagree. NULL stays NULL.

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, BinaryBuilder};
use datafusion::arrow::datatypes::DataType;
use datafusion::logical_expr::{create_udf, ColumnarValue, ScalarUDF, Volatility};

pub const NATURAL: &str = "qh_natural";

pub fn natural_udf() -> ScalarUDF {
    create_udf(
        NATURAL,
        vec![DataType::Utf8],
        DataType::Binary,
        Volatility::Immutable,
        Arc::new(|args: &[ColumnarValue]| {
            let arrays = ColumnarValue::values_to_arrays(args)?;
            let text = arrays[0].as_string::<i32>();
            let mut out = BinaryBuilder::with_capacity(text.len(), text.len() * 16);
            let mut key = Vec::new();
            for row in 0..text.len() {
                if text.is_null(row) {
                    out.append_null();
                } else {
                    key.clear();
                    qh_result_store::natural_key(text.value(row), &mut key);
                    out.append_value(&key);
                }
            }
            Ok(ColumnarValue::Array(Arc::new(out.finish()) as ArrayRef))
        }),
    )
}
