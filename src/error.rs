use std::process;

#[derive(Debug, thiserror::Error)]
pub enum ItrError {
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

    #[error("No .itr.db found. Run 'itr init' to create one.")]
    NoDatabase,

    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl ItrError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ItrError::NotFound(_) => 1,
            ItrError::CycleDetected(_) => 1,
            ItrError::InvalidValue { .. } => 1,
            ItrError::NoDatabase => 1,
            ItrError::Db(_) => 1,
            ItrError::Parse(_) => 1,
            ItrError::Io(_) => 1,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            ItrError::NotFound(_) => "NOT_FOUND",
            ItrError::CycleDetected(_) => "CYCLE_DETECTED",
            ItrError::InvalidValue { .. } => "INVALID_VALUE",
            ItrError::NoDatabase => "NO_DATABASE",
            ItrError::Db(_) => "DB_ERROR",
            ItrError::Parse(_) => "PARSE_ERROR",
            ItrError::Io(_) => "IO_ERROR",
        }
    }
}

pub fn handle_error(err: ItrError, json_mode: bool) -> ! {
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
