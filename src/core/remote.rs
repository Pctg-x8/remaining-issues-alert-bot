
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::borrow::Cow;
use rand::{Rng, seq::SliceRandom};
use std::sync::Arc;

pub struct RemoteResources {
    pub repo: RemoteRepository<'static>,
    pub ghclient: RemoteProjectHub,
    pub persistent: super::Persistent
}
impl RemoteResources {
    pub async fn new(database_url: &str, redis_url: &str) -> Self {
        let ghclient = RemoteProjectHub::new();
        let persistent = super::Persistent::new(database_url, redis_url);
        let repo = ghclient.repository("Pctg-x8/peridot").await;

        RemoteResources { repo, ghclient, persistent }
    }

    pub async fn collect_remaining_tasks<'s>(&self, user: super::User<'s>) -> super::ProcessResult<Vec<CurrentTaskState>> {
        let (_, ghlogin) = user.identify_as_github_id(&self.persistent)?;
        let works = self.repo.opened_issues_by_assignee(&ghlogin).await?.unwrap().into_iter()
            .map(|s| async {
                let exinfo = self.repo.issue_on_board(s.number).await?.unwrap();
                let unchecked_count = s.body.matches("- [ ]").count();
                let checked_count = s.body.matches("- [x]").count();
                let total_checks = unchecked_count + checked_count;

                Ok(CurrentTaskState {
                    number: s.number, title: s.title, url: s.html_url, progress: (checked_count, total_checks),
                    is_pr: s.pull_request.is_some(), pipeline_stage_name: exinfo.pipeline.name
                })
            });
        futures_util::future::join_all(works).await.into_iter().collect::<Result<Vec<_>, _>>()
    }

    pub async fn greeting<'s>(&self, user: super::User<'s>, following_desumasu: bool) -> super::ProcessResult<String> {
        let (ghid, ghlogin) = user.identify_as_github_id(&self.persistent)?;
        if self.persistent.is_user_working(ghid) {
            self.greeting_message(None, following_desumasu)
        } else {
            let tasklist = self.collect_remaining_tasks(super::User::GitHub(ghid, ghlogin)).await?;
            self.persistent.user_setup_work(ghid, super::utcnow(), &tasklist.iter().map(|x| {
                let (a, b) = x.progress;
                super::RemainingWork { issue_id: x.number as _, progress: Some((a as _, b as _)) }
            }).collect::<Vec<_>>());
            return self.greeting_message(Some(tasklist), following_desumasu);
        }
    }
    pub fn workend(&self, user: super::User) -> super::ProcessResult<String> {
        // let (ghid, ghlogin) = user.identify_as_github_id(&self.persistent)?;
        /*let user_remaining_tasks: Vec<_> = self.collect_remaining_tasks(user)?
            .into_iter().map(|x| TaskAchievementWriter(x).to_string()).collect();*/
        self.workend_message()
    }

    pub fn greeting_message(&self, tasklist: Option<Vec<CurrentTaskState>>, following_desumasu: bool) -> super::ProcessResult<String> {
        let &hello_text1 = if following_desumasu {
            ["おはようございます！", "おはようございまーす！"].choose(&mut rand::thread_rng()).unwrap()
        } else {
            ["おはよう！", "おはよ！"].choose(&mut rand::thread_rng()).unwrap()
        };
        if let Some(tasklist) = tasklist {
            let (hello_text2, tasklines, after_text);
            if tasklist.is_empty() {
                hello_text2 = *if following_desumasu {
                    ["今日のタスクは...0です！", "残ってるお仕事はありませんよー。"].choose(&mut rand::thread_rng()).unwrap()
                } else {
                    ["今残ってるタスクはないかな。", "今日の献立は......えーっと......ないかも"]
                        .choose(&mut rand::thread_rng()).unwrap()
                };
                tasklines = Vec::new();
                after_text = *if following_desumasu {
                    [
                        "それじゃ、今日も張り切って行きましょー！！",
                        "毎日お疲れ様、今日も一日頑張って行きましょっ！"
                    ].choose(&mut rand::thread_rng()).unwrap()
                } else {
                    [
                        "それじゃ、今日もはりきっていこー！！",
                        "いつもお疲れ様。今日も頑張ってね！！",
                        "毎日頑張ってるね！今日も無理せずいこうね！！"
                    ].choose(&mut rand::thread_rng()).unwrap()
                };
            } else {
                hello_text2 = *if following_desumasu {
                    ["今日のタスクはこれですよー！", "残りのお仕事はこちらでーす。"].choose(&mut rand::thread_rng()).unwrap()
                } else {
                    ["今残ってるタスクはこれだよ。", "今日の献立だよー:issue-o::pr::issue-o::pr:"]
                        .choose(&mut rand::thread_rng()).unwrap()
                };
                tasklines = tasklist.into_iter().map(|x| TaskAchievementWriter(x).to_string()).collect();
                after_text = *if following_desumasu {
                    [
                        "それじゃ、今日も張り切って行きましょー！！",
                        "毎日お疲れ様、今日も一日頑張って行きましょっ！",
                        "まだまだ課題は山積みですよ！でもほどほどにこなしていきましょうね！"
                    ].choose(&mut rand::thread_rng()).unwrap()
                } else {
                    [
                        "それじゃ、今日もはりきっていこー！！",
                        "いつもお疲れ様。今日も頑張ってね！！",
                        "毎日頑張ってるね！今日も無理せずいこうね！！"
                    ].choose(&mut rand::thread_rng()).unwrap()
                };
            }

            Ok(format!("{}{}\n{}\n{}", hello_text1, hello_text2, tasklines.join("\n"), after_text))
        } else {
            Ok(hello_text1.to_owned())
        }
    }
    pub fn workend_message(&self) -> super::ProcessResult<String> {
        let begin_text = ["今日も一日お疲れさま！"].choose(&mut rand::thread_rng()).unwrap();
        let nodata_text = "";

        Ok(format!("{}{}", begin_text, nodata_text))
    }
    pub async fn count_source<'s, 'u>(
        &self, what: &'s super::TellWhat, user: super::User<'u>
    ) -> super::ProcessResult<(usize, &'s str, bool)> {
        match what {
            super::TellWhat::Issues { unresolved: true, user_calls, ask_for_remains } =>
                Ok((self.repo.opened_issues_by_labeled("UnresolvedQuestions").await?.unwrap().len(), user_calls as _, *ask_for_remains)),
            super::TellWhat::Tasks(user_calls, afr, _) =>
                Ok((self.collect_remaining_tasks(user).await?.len(), user_calls as _, *afr)),
            _ => Err(super::ProcessError::UncountableObjective)
        }
    }
    pub fn report_count(&self, count: usize, user_calls: &str, ask_for_remains: bool) -> String {
        format!("{}は{}{}個だよー", user_calls, if ask_for_remains { "あと" } else { "" }, count)
    }
    pub async fn report_tasks<'s>(&self, user_calls: &str, user: super::User<'s>) -> super::ProcessResult<String> {
        let tasklines: Vec<_> = self.collect_remaining_tasks(user).await?.into_iter().map(TaskAchievementWriter)
            .map(|x| x.to_string()).collect();
        
        Ok(format!("{}は以下の通りだよ:\n{}", user_calls, tasklines.join("\n")))
    }
    pub async fn report_pullrequests(&self, user_calls: &str, opened_assignee: bool, unreviewed: bool) -> surf::Result<Cow<'static, str>> {
        let prs = self.repo.pullrequests().await?.unwrap();
        if !opened_assignee && !unreviewed {
            // random
            let pr = prs.choose(&mut rand::thread_rng());
            Ok(if let Some(pr) = pr {
                format!("はい。\n{}", OpenedPullRequestFormatter::new(pr, &self.repo).await).into()
            } else {
                Cow::from(*[
                    "開いてるPRはないみたい", "開いてるPRはないみたいだね。その調子！",
                    "うーん、ちょっと見当たらないかも......？", "今は届いてなさそうだね"
                ].choose(&mut rand::thread_rng()).unwrap())
            })
        } else {
            let filtered_elements = prs.iter().filter(|p| {
                (opened_assignee && p.assignees.iter().find(|a| a.login == "Pctg-x8").is_some()) ||
                (unreviewed && p.requested_reviewers.iter().find(|a| a.login == "Pctg-x8").is_some())
            });
            let filtered = futures_util::future::join_all(filtered_elements.map(|p| OpenedPullRequestFormatter::new(p, &self.repo))).await;

            if filtered.is_empty() {
                let mut haystack: Vec<Cow<str>> = vec![
                    format!("{}はないみたい", user_calls).into(),
                    format!("{}はないみたいだね。その調子！", user_calls).into(),
                    "うーん、ちょっと見当たらないかも......？".into(),
                    "今は届いてなさそうだね".into()
                ];
                let index = rand::thread_rng().gen_range(0, haystack.len());

                Ok(haystack.remove(index))
            } else {
                let msg = format!("{}はこれかなー\n{}", user_calls,
                    filtered.into_iter().map(|s| s.to_string()).collect::<Vec<_>>().join("\n"));
                Ok(msg.into())
            }
        }
    }
    pub async fn report_unresolved_issues(&self) -> surf::Result<Cow<'static, str>> {
        let issues = self.repo.unresolved_issues().await?.unwrap();
        if issues.is_empty() { Ok("未解決の課題はないかなぁ".into()) }
        else {
            let headtext = "まだ解決してないことがあるのは以下の課題だよー。忘れないようにね！";
            let issuelines = issues.into_iter().map(|s| async {
                let exinfo = self.repo.issue_on_board(s.number).await.unwrap().unwrap();
                let unchecked_count = s.body.matches("- [ ]").count();
                let checked_count = s.body.matches("- [x]").count();
                let total_checks = unchecked_count + checked_count;

                TaskUnresolvedCountWriter(CurrentTaskState {
                    number: s.number, title: s.title, url: s.html_url, progress: (checked_count, total_checks),
                    is_pr: s.pull_request.is_some(), pipeline_stage_name: exinfo.pipeline.name
                }).to_string()
            }).collect::<futures_util::future::JoinAll<_>>().await;

            Ok(format!("{}\n{}", headtext, issuelines.join("\n")).into())
        }
    }
}

pub struct CurrentTaskState {
    number: usize, title: String, url: String, progress: (usize, usize), is_pr: bool,
    pipeline_stage_name: String
}

pub struct TaskAchievementWriter(CurrentTaskState);
impl Display for TaskAchievementWriter {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        let stage_str =
            if self.0.pipeline_stage_name == "In Progress" { format!("*{} ステージ*", self.0.pipeline_stage_name) }
            else { format!("{} ステージ", self.0.pipeline_stage_name) };
        let miscinfo =
            if self.0.progress.1 == 0 { stage_str }
            else { format!("{}, {}個中{}個完了", stage_str, self.0.progress.1, self.0.progress.0) };
        let icon = if self.0.is_pr { "pr" } else { "issue-o" };

        write!(fmt, "<{}|:{}:#{}>: {} ({})", self.0.url, icon, self.0.number, self.0.title, miscinfo)
    }
}
pub struct TaskUnresolvedCountWriter(CurrentTaskState);
impl Display for TaskUnresolvedCountWriter {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        let stage_str =
            if self.0.pipeline_stage_name == "In Progress" { format!("*{} ステージ*", self.0.pipeline_stage_name) }
            else { format!("{} ステージ", self.0.pipeline_stage_name) };
        let miscinfo =
            if self.0.progress.1 == 0 { stage_str }
            else { format!("{}, {}個中{}個未解決", stage_str, self.0.progress.1, self.0.progress.1 - self.0.progress.0) };
        let icon = if self.0.is_pr { "pr" } else { "issue-o" };

        write!(fmt, "<{}|:{}:#{}>: {} ({})", self.0.url, icon, self.0.number, self.0.title, miscinfo)
    }
}

pub struct OpenedPullRequestFormatter<'p>(&'p PullRequestInfo, ExtraIssueInfo, usize, usize);
impl<'p> Display for OpenedPullRequestFormatter<'p> {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        let stage_str = if self.1.pipeline.name == "In Progress" {
            format!("*{} ステージ*", self.1.pipeline.name)
        } else {
            format!("{} ステージ", self.1.pipeline.name)
        };
        let miscinfo =
            if self.3 == 0 { stage_str }
            else { format!("{}, {}個中{}個完了", stage_str, self.2, self.3) };
        
        write!(fmt, "<{}|#{}>: {} ({})", self.0.html_url, self.0.number, self.0.title, miscinfo)
    }
}
impl<'p> OpenedPullRequestFormatter<'p> {
    async fn new<'r>(pr: &'p PullRequestInfo, repo: &RemoteRepository<'r>) -> OpenedPullRequestFormatter<'p> {
        let einfo = repo.issue_on_board(pr.number).await.unwrap().unwrap();
        let unchecked_count = pr.body.matches("- [ ]").count();
        let checked_count = pr.body.matches("- [x]").count();
        let total_checks = unchecked_count + checked_count;

        OpenedPullRequestFormatter(pr, einfo, checked_count, total_checks)
    }
}

fn github_api_token() -> String { std::env::var("GITHUB_API_TOKEN").expect("GITHUB_API_TOKEN not set") }
fn zenhub_api_token() -> String { std::env::var("ZENHUB_API_TOKEN").expect("ZENHUB_API_TOKEN not set") }

pub struct RemoteProjectHubGate { c: surf::Client, auth: String }
#[derive(Clone)]
pub struct RemoteProjectHub(Arc<RemoteProjectHubGate>);
impl RemoteProjectHub {
    pub fn new() -> Self {
        RemoteProjectHub(Arc::new(RemoteProjectHubGate {
            c: surf::Client::new(), auth: format!("token {}", github_api_token())
        }))
    }

    /// Requests an authorized request to GitHub
    fn github_auth_get(&self, url: impl AsRef<str>) -> surf::RequestBuilder {
        self.0.c.get(url).header(surf::http::headers::AUTHORIZATION, &self.0.auth[..])
    }
    /// Requests an authorized request to ZenHub
    fn zenhub_auth_get(&self, url: impl AsRef<str>) -> surf::RequestBuilder {
        self.0.c.get(url).header("X-Authentication-Token", zenhub_api_token())
    }
}

#[derive(serde::Deserialize, Debug)]
pub struct RepositoryInfo {
    pub id: usize
}
#[derive(serde::Deserialize, Debug)]
pub struct PullRequestUrls { pub html_url: String }
#[derive(Debug, serde::Deserialize)]
pub struct IssueInfo {
    pub html_url: String, pub number: usize, pub title: String, pub body: String,
    pub pull_request: Option<PullRequestUrls>
}
#[derive(Debug, serde::Deserialize)]
pub struct PullRequestInfo {
    pub number: usize, pub html_url: String, pub title: String, pub body: String,
    pub assignees: Vec<UserInfo>, pub requested_reviewers: Vec<UserInfo>
}
#[derive(Debug, serde::Deserialize)]
pub struct UserInfo {
    pub login: String, pub id: u64
}
#[derive(Debug, serde::Deserialize)]
pub struct FullUserInfo {
    pub login: String, pub id: u64, pub name: String
}
#[derive(Debug, serde::Deserialize)]
pub struct ExtraIssueInfo {
    pub is_epic: bool, pub pipeline: PipelineInfo
}
#[derive(Debug, serde::Deserialize)]
pub struct PipelineInfo { pub name: String }

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
pub enum GitHubResponse<T> { Success(T), Failed { message: String } }
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
    #[cfg(not(feature = "offline-check"))]
    pub async fn repository<'g>(&self, repo: &'g str) -> RemoteRepository<'g> {
        let repoinfo: RepositoryInfo = self.github_auth_get(&format!("https://api.github.com/repos/{}", repo))
            .send().await.unwrap().body_json().await.unwrap();
        RemoteRepository(self.clone(), repo, repoinfo.id)
    }
    #[cfg(feature = "offline-check")]
    pub async fn repository<'g>(&self, repo: &'g str) -> RemoteRepository<'g> {
        RemoteRepository(self.clone(), repo, 2345678)
    }
}
impl<'g> RemoteRepository<'g> {
    pub async fn opened_issues_by_assignee(&self, assignee: &str) -> surf::Result<GitHubResponse<Vec<IssueInfo>>> {
        self.0.github_auth_get(format!("https://api.github.com/repos/{}/issues", self.1))
            .query(&[("assignee", assignee), ("state", "open"), ("sort", "updated"), ("direction", "asc")])?
            .send().await?
            .body_json().await
    }
    pub async fn opened_issues_by_labeled(&self, labels: &str) -> surf::Result<GitHubResponse<Vec<IssueInfo>>> {
        self.0.github_auth_get(format!("https://api.github.com/repos/{}/issues", self.1))
            .query(&[("labels", labels), ("state", "open"), ("sort", "updated"), ("direction", "asc")])?
            .send().await?
            .body_json().await
    }
    pub async fn issue_on_board(&self, num: usize) -> surf::Result<GitHubResponse<ExtraIssueInfo>> {
        self.0.zenhub_auth_get(format!("https://api.zenhub.io/p1/repositories/{}/issues/{}", self.2, num))
            .send().await?
            .body_json().await
    }
    pub async fn pullrequests(&self) -> surf::Result<GitHubResponse<Vec<PullRequestInfo>>> {
        self.0.github_auth_get(format!("https://api.github.com/repos/{}/pulls", self.1))
            .send().await?
            .body_json().await
    }
    pub async fn unresolved_issues(&self) -> surf::Result<GitHubResponse<Vec<IssueInfo>>> {
        self.opened_issues_by_labeled("UnresolvedQuestions").await
    }
}

pub struct RemoteUser(RemoteProjectHub, FullUserInfo);
impl RemoteProjectHub {
    pub async fn user(&self, user_login_name: &str) -> surf::Result<GitHubResponse<RemoteUser>> {
        let resp: GitHubResponse<_> = self.github_auth_get(format!("https://api.github.com/users/{}", user_login_name))
            .send().await?
            .body_json().await?;
        Ok(resp.map(|r| RemoteUser(self.clone(), r)))
    }
}
impl RemoteUser {
    pub fn response(&self) -> &FullUserInfo { &self.1 }
}
