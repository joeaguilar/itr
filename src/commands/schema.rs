use crate::db;
use crate::error::ItrError;
use crate::format::Format;

pub fn run(fmt: Format) -> Result<(), ItrError> {
    let schema = db::get_schema_sql();

    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "schema": schema });
            println!("{}", out);
        }
        _ => {
            println!("{}", schema);
        }
    }

    Ok(())
}
