use std::process;

#[derive(Debug, thiserror::Error)]
pub enum NitError {
    #[error("Issue {0} not found")]
    NotFound(i64),

    #[error("Cycle detected: {0}")]
    CycleDetected(String),

    #[error("Invalid value for {field}: '{value}'. Valid: {valid}")]
    InvalidValue {
        field: String,
        value: String,
        valid: String,
    },

    #[error("No .nit.db found. Run 'nit init' to create one.")]
    NoDatabase,

    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl NitError {
    pub fn exit_code(&self) -> i32 {
        match self {
            NitError::NotFound(_) => 1,
            NitError::CycleDetected(_) => 1,
            NitError::InvalidValue { .. } => 1,
            NitError::NoDatabase => 1,
            NitError::Db(_) => 1,
            NitError::Parse(_) => 1,
            NitError::Io(_) => 1,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            NitError::NotFound(_) => "NOT_FOUND",
            NitError::CycleDetected(_) => "CYCLE_DETECTED",
            NitError::InvalidValue { .. } => "INVALID_VALUE",
            NitError::NoDatabase => "NO_DATABASE",
            NitError::Db(_) => "DB_ERROR",
            NitError::Parse(_) => "PARSE_ERROR",
            NitError::Io(_) => "IO_ERROR",
        }
    }
}

pub fn handle_error(err: NitError, json_mode: bool) -> ! {
    if json_mode {
        let err_json = serde_json::json!({
            "error": err.to_string(),
            "code": err.error_code(),
        });
        eprintln!("{}", err_json);
    } else {
        eprintln!("ERROR: {}", err);
    }
    process::exit(err.exit_code());
}

/// Exit with code 2 for empty result sets.
pub fn exit_empty(json_mode: bool, msg: &str) -> ! {
    if json_mode {
        // For json mode, output empty array on stdout
        println!("[]");
    } else {
        eprintln!("{}", msg);
    }
    process::exit(2);
}
