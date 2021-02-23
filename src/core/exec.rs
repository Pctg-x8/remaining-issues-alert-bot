//! Bot Core: Executor

use std::sync::Arc;
use std::borrow::Cow;
use super::jparse::{Command, TellWhat};
use rand::seq::SliceRandom;

pub struct CommandExecutor {
    remote: Arc<super::RemoteResources>
}
impl CommandExecutor {
    pub fn new(remote: &Arc<super::RemoteResources>) -> Self {
        CommandExecutor {
            remote: remote.clone()
        }
    }
    
    pub async fn first_contact(&self, github_username: &str, slack_userid: &str) -> super::ProcessResult<String> {
        match self.remote.ghclient.user(github_username).await.map_err(super::ProcessError::GithubAPIError)? {
            super::GitHubResponse::Success(u) => {
                self.remote.persistent.init_user(u.response().id as _, slack_userid, &u.response().login);
                let &(pretext, posttext) = [
                    ("はじめまして！", "...さん？"),
                    ("わぁー、", "さん？はじめまして！"),
                    ("", "さん！これからよろしくね。")
                ].choose(&mut rand::thread_rng()).unwrap();
                Ok(format!("{}{}({}){}", pretext, github_username, u.response().name, posttext))
            },
            super::GitHubResponse::Failed { message } if message == "Not Found" => Ok(String::from("誰......？")),
            super::GitHubResponse::Failed { message } => Ok(format!("GitHubResponse Failed: {}", message))
        }
    }
    pub async fn execute<'s>(
        &self, cmd: crate::core::Command, target_user: User<'s>
    ) -> super::ProcessResult<Cow<'static, str>> {
        log::debug!("CommandTree: {:?}", cmd);

        match cmd {
            Command::Greeting(following_desumasu) => self.remote.greeting(target_user, following_desumasu)
                .await.map(Cow::Owned),
            Command::Tell(None) => Ok(Cow::Owned(String::from("何について知りたいの？"))),
            Command::Tell(Some(tw)) => match tw {
                TellWhat::RandomNumber => Ok(Cow::Owned(rand::random::<i64>().to_string())),
                // 明日の/昨日のタスクを取得する必要性がわからないのでひとまず無視
                TellWhat::Tasks(user_calls, _, _) => self.remote.report_tasks(&user_calls, target_user).await
                    .map(Cow::Owned),
                TellWhat::CountOf(tw) => self.remote.count_source(&tw, target_user).await
                    .map(|(count, user_calls, ask_for_remains)| Cow::Owned(
                        self.remote.report_count(count, user_calls, ask_for_remains)
                    )),
                TellWhat::Issues { unresolved: true, .. } => self.remote.report_unresolved_issues().await.map_err(super::ProcessError::from),
                TellWhat::Issues { unresolved: false, .. } => Ok(Cow::Borrowed("Issues(unresolved=false)?")),
                TellWhat::PullRequests { opened_assignee, unreviewed, user_calls } => self.remote.report_pullrequests(
                    &user_calls, opened_assignee, unreviewed
                ).await.map_err(super::ProcessError::from)
            },
            Command::EndWorking(_at, _dayoffset) => self.remote.workend(target_user).map(Cow::Owned)
        }
    }
}

pub enum User<'s> { Slack(&'s str), GitHub(i32, Cow<'s, str>) }
impl<'s> User<'s> {
    #[cfg(feature = "offline-check")]
    pub fn identify_as_github_id(self, _persistent: &super::Persistent) -> super::ProcessResult<(i32, Cow<'s, str>)> {
        match self {
            User::Slack(_) => Ok((1234567, Cow::Borrowed("GitHubMockUser"))),
            User::GitHub(id, login) => Ok((id, login))
        }
    }
    #[cfg(not(feature = "offline-check"))]
    pub fn identify_as_github_id(self, persistent: &super::Persistent) -> super::ProcessResult<(i32, Cow<'s, str>)> {
        match self {
            User::Slack(skid) => persistent.query_github_from_slack_id(skid).map(|(a, b)| (a, b.into()))
                .ok_or(super::ProcessError::UserUnidentifiable),
            User::GitHub(id, login) => Ok((id, login))
        }
    }
}
