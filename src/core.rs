
#[derive(Debug)]
pub enum ProcessError {
    UserUnidentifiable, UncountableObjective, Network(surf::Error), RecognizationFailure(String),
    GithubAPIError(surf::Error)
}
impl From<surf::Error> for ProcessError { fn from(v: surf::Error) -> Self { ProcessError::Network(v) } }
type ProcessResult<T> = Result<T, ProcessError>;

fn utcnow() -> chrono::NaiveDateTime { chrono::Utc::now().naive_utc() }

mod persistent; pub use self::persistent::*;
mod jparse; pub use self::jparse::*;
mod remote; pub use self::remote::*;
mod exec; pub use self::exec::*;