
use reqwest as rq;
use std::sync::Arc;

fn github_api_token() -> &'static str { env!("GITHUB_API_TOKEN") }
fn zenhub_api_token() -> &'static str { env!("ZENHUB_API_TOKEN") }

pub struct RemoteProjectHubGate { c: rq::Client, auth: String }
#[derive(Clone)]
pub struct RemoteProjectHub(Arc<RemoteProjectHubGate>);
impl RemoteProjectHub {
    pub fn new() -> Self {
        RemoteProjectHub(Arc::new(RemoteProjectHubGate { c: rq::Client::new(), auth: format!("token {}", github_api_token()) }))
    }

    /// Requests an authorized request to GitHub
    fn github_auth_get<Url: rq::IntoUrl>(&self, url: Url) -> rq::RequestBuilder {
        self.0.c.get(url).header(rq::header::AUTHORIZATION, &self.0.auth[..])
    }
    /// Requests an authorized request to ZenHub
    fn zenhub_auth_get<Url: rq::IntoUrl>(&self, url: Url) -> rq::RequestBuilder {
        self.0.c.get(url).header("X-Authentication-Token", zenhub_api_token())
    }
}

#[derive(Deserialize, Debug)] pub struct RepositoryInfo {
    pub id: usize
}
#[derive(Deserialize, Debug)] pub struct PullRequestUrls { pub html_url: String }
#[derive(Debug, Deserialize)] pub struct IssueInfo {
    pub html_url: String, pub number: usize, pub title: String, pub body: String,
    pub pull_request: Option<PullRequestUrls>
}
#[derive(Debug, Deserialize)] pub struct PullRequestInfo {
    pub number: usize, pub html_url: String, pub title: String, pub body: String,
    pub assignees: Vec<UserInfo>, pub requested_reviewers: Vec<UserInfo>
}
#[derive(Debug, Deserialize)] pub struct UserInfo {
    pub login: String, pub id: u64
}
#[derive(Debug, Deserialize)] pub struct FullUserInfo {
    pub login: String, pub id: u64, pub name: String
}
#[derive(Debug, Deserialize)] pub struct ExtraIssueInfo {
    pub is_epic: bool, pub pipeline: PipelineInfo
}
#[derive(Debug, Deserialize)] pub struct PipelineInfo { pub name: String }

#[derive(Debug, Deserialize)] #[serde(untagged)] pub enum GitHubResponse<T> {
    Success(T), Failed { message: String }
}
impl<T> GitHubResponse<T> {
    pub fn map<F, U>(self, mapper: F) -> GitHubResponse<U> where F: FnOnce(T) -> U {
        match self {
            GitHubResponse::Success(v) => GitHubResponse::Success(mapper(v)),
            GitHubResponse::Failed { message } => GitHubResponse::Failed { message }
        }
    }
    pub fn unwrap(self) -> T {
        match self {
            GitHubResponse::Success(v) => v,
            GitHubResponse::Failed { message } => panic!("Unwrapping GitHubResponse Failed! {}", message)
        }
    }
}

// QueryBuilder
pub struct RemoteRepository<'s>(RemoteProjectHub, &'s str, usize);
impl RemoteProjectHub {
    pub fn repository<'g>(&self, repo: &'g str) -> RemoteRepository<'g> {
        #[cfg(not(feature = "offline-check"))] {
            let repoinfo: RepositoryInfo = self.github_auth_get(&format!("https://api.github.com/repos/{}", repo))
                .send().unwrap().json().unwrap();
            RemoteRepository(self.clone(), repo, repoinfo.id)
        }
        #[cfg(feature = "offline-check")] RemoteRepository(self.clone(), repo, 2345678)
    }
}
impl<'g> RemoteRepository<'g> {
    pub fn opened_issues_by_assignee(&self, assignee: &str) -> rq::Result<GitHubResponse<Vec<IssueInfo>>> {
        self.0.github_auth_get(&format!("https://api.github.com/repos/{}/issues", self.1))
            .query(&[("assignee", assignee), ("state", "open"), ("sort", "updated"), ("direction", "asc")])
            .send()?.json()
    }
    pub fn opened_issues_by_labeled(&self, labels: &str) -> rq::Result<GitHubResponse<Vec<IssueInfo>>> {
        self.0.github_auth_get(&format!("https://api.github.com/repos/{}/issues", self.1))
            .query(&[("labels", labels), ("state", "open"), ("sort", "updated"), ("direction", "asc")])
            .send()?.json()
    }
    #[cfg(feature = "offline-check")]
    pub fn issue_on_board(&self, num: usize) -> rq::Result<GitHubResponse<ExtraIssueInfo>> {
        Ok(GitHubResponse::Success(ExtraIssueInfo {
            is_epic: false, pipeline: PipelineInfo { name: "Backlog".to_owned() }
        }))
    }
    #[cfg(not(feature = "offline-check"))]
    pub fn issue_on_board(&self, num: usize) -> rq::Result<GitHubResponse<ExtraIssueInfo>> {
        self.0.zenhub_auth_get(&format!("https://api.zenhub.io/p1/repositories/{}/issues/{}", self.2, num))
            .send()?.json()
    }
    pub fn pullrequests(&self) -> rq::Result<GitHubResponse<Vec<PullRequestInfo>>> {
        self.0.github_auth_get(&format!("https://api.github.com/repos/{}/pulls", self.1))
            .send()?.json()
    }

    #[cfg(not(feature = "offline-check"))]
    pub fn unresolved_issues(&self) -> rq::Result<GitHubResponse<Vec<IssueInfo>>> {
        self.opened_issues_by_labeled("UnresolvedQuestions")
    }
    #[cfg(feature = "offline-check")]
    pub fn unresolved_issues(&self) -> rq::Result<GitHubResponse<Vec<IssueInfo>>> {
        Ok(GitHubResponse::Success(vec![IssueInfo {
            number: 3, html_url: "http://github.com/Pctg-x8/peridot/issues/3".to_owned(),
            title: "テスト未解決".to_owned(), body: "- [ ] テスト項目1\n- [x] テスト項目2".to_owned(),
            pull_request: None
        }]))
    }
}

pub struct RemoteUser(RemoteProjectHub, FullUserInfo);
impl RemoteProjectHub {
    pub fn user(&self, user_login_name: &str) -> rq::Result<GitHubResponse<RemoteUser>> {
        self.github_auth_get(&format!("https://api.github.com/users/{}", user_login_name))
            .send()?.json().map(|j: GitHubResponse<_>| j.map(|jm| RemoteUser(self.clone(), jm)))
    }
}
impl RemoteUser {
    pub fn response(&self) -> &FullUserInfo { &self.1 }
}
