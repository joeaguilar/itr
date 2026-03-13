use crate::agent_docs::AGENT_DOCS;
use crate::error::ItrError;
use crate::format::Format;

#[allow(clippy::unnecessary_wraps)]
pub fn run(fmt: Format) -> Result<(), ItrError> {
    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "guide": AGENT_DOCS });
            println!("{}", out);
        }
        _ => {
            print!("{}", AGENT_DOCS);
        }
    }

    Ok(())
}
