
use reqwest as rq;

#[derive(Debug)]
pub enum ProcessError {
    UserUnidentifiable, UncountableObjective, Network(rq::Error), RecognizationFailure(String),
    GithubAPIError(rq::Error)
}
impl From<rq::Error> for ProcessError { fn from(v: rq::Error) -> Self { ProcessError::Network(v) } }
type ProcessResult<T> = Result<T, ProcessError>;

fn utcnow() -> chrono::NaiveDateTime { chrono::Utc::now().naive_utc() }

mod persistent; pub use self::persistent::*;
mod jparse; pub use self::jparse::*;
mod remote; pub use self::remote::*;
mod exec; pub use self::exec::*;