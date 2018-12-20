
extern crate libc;
extern crate asyncslackbot;
use asyncslackbot::*;
extern crate reqwest;
extern crate serde; #[macro_use] extern crate serde_derive; extern crate serde_json;
extern crate regex; extern crate rand;
use rand::Rng; use regex::Regex;
use std::collections::BTreeMap;

mod cabocha; use cabocha as cabo;
use std::borrow::Cow;

mod backend; use backend::*;
extern crate chrono;
extern crate diesel;
extern crate r2d2;
extern crate redis;
extern crate r2d2_redis;
mod persistent; use persistent::*;
use reqwest as rq;

mod sched; use sched::{Scheduler, CancellationToken};

#[cfg(feature = "offline-check")]
mod readline;
#[cfg(feature = "offline-check")]
use readline::Readline;
use std::io::Write;

#[cfg(not(feature = "offline-check"))]
fn slack_api_token() -> &'static str { env!("SLACK_API_TOKEN") }

#[derive(Debug)] #[cfg(feature = "offline-check")]
pub struct SlackWebApi2 {
    endpoint: &'static str, paramdata: String
}
#[derive(Deserialize, Debug)] #[cfg(feature = "offline-check")]
pub struct PostMessageParams<'s> {
    pub channel: &'s str, pub text: Cow<'s, str>,
    pub as_user: Option<bool>, pub icon_emoji: Option<&'s str>, pub icon_url: Option<&'s str>, pub username: Option<&'s str>,
    pub attachments: Vec<Attachment<'s>>
}
#[derive(Deserialize, Debug)] #[cfg(feature = "offline-check")]
pub struct Attachment<'s> { pub color: Option<Cow<'s, str>>, pub text: Option<Cow<'s, str>> }

fn main() {
    println!("SlackBot[Remaining Issues Alerting]");

    #[cfg(not(feature = "offline-check"))] launch_rtm::<BotLogic>(slack_api_token());
    #[cfg(feature = "offline-check")] mock_rtm::<BotLogic>();
}

#[cfg(feature = "offline-check")]
fn mock_rtm<L: SlackBotLogic>() {
    let (sender, recv) = std::sync::mpsc::channel();
    let th = std::thread::spawn(move || {
        while let Ok(d) = recv.recv() {
            println!("[ApiSenderOffline] SendRequest = {:?}", d);
            let dp = unsafe { std::mem::transmute::<&SlackWebApiCall, &SlackWebApi2>(&d) };
            if dp.endpoint.ends_with("/chat.postMessage") {
                let params: PostMessageParams = serde_json::from_str(&dp.paramdata).unwrap();
                println!("\x1b[1m< {}\x1b[0m", params.text);
                for p in &params.attachments {
                    if p.color.as_ref().map_or(false, |s| s == "danger") {
                        if let Some(ref s) = p.text {
                            println!("\x1b[31m<| {}\x1b[0m", s);
                        }
                    }
                }
            }
        }
        println!("[ApiSenderOffline] Shutdown");
    });
    let sender: AsyncSlackApiSender = unsafe { std::mem::transmute(sender) };
    let mut logic = L::launch(&sender, &ConnectionAccountInfo {
        id: "UBOTDUMMY".to_owned(), name: "koyuki".to_owned()
    }, &TeamInfo {
        id: "TRTMEMU".to_owned(), name: "RtmOffliceEmu".to_owned(), domain: "localhost".to_owned()
    });
    let rl = Readline::init(None);
    while let Some(line) = rl.readline("> ") {
        logic.on_message(&sender, MessageEvent {
            text: std::borrow::Cow::Borrowed(line.as_str().unwrap()),
            channel: "DDUMMYCH", ts: "123456789", user: "UDUMMYSENDER",
            subtype: None
        });
    }
}

mod jparse; use jparse::*;
use std::sync::Arc;

pub struct UserNotifyHandler { remote: Arc<RemoteResources>, api: AsyncSlackApiSender }
impl sched::EventHandler<UserNotify> for UserNotifyHandler {
    fn handle_batch(&self, events: Vec<Option<UserNotify>>) {
        for e in events.into_iter().filter_map(|x| x) {
            println!("Notify: {:?}", e);
            match e {
                UserNotify::Hello(skid, ghid, ghl) => match self.remote.greeting(User::GitHub(ghid, ghl.into()), false) {
                    Ok(m) => general_reply_to_id(&self.api, &skid, &m),
                    Err(er) => { eprintln!("An error occured while generating hello message: {:?}", er); }
                },
                UserNotify::ByeReport(skid, ghid, ghl) => match self.remote.workend(User::GitHub(ghid, ghl.into())) {
                    Ok(m) => general_reply_to_id(&self.api, &skid, &m),
                    Err(er) => { eprintln!("An error occured while generating workend message: {:?}", er); }
                }
            }
        }
    }
}

#[derive(Debug)] pub enum UserNotify { Hello(String, i32, String), ByeReport(String, i32, String) }
pub struct RemoteResources {
    repo: RemoteRepository<'static>,
    ghclient: RemoteProjectHub, persistent: Persistent
}
pub struct BotLogic {
    remote: Arc<RemoteResources>, api: AsyncSlackApiSender, bot_userid: String,
    call_checker: CallingSentenceDetector, rewriter: Rewriter<'static>,
    dparse: JpDepsParser, greeting_rx: Regex, sched: Scheduler<UserNotify, UserNotifyHandler>,
    intime_cancellation_token: BTreeMap<i32, CancellationToken>,
    outtime_cancellation_token: BTreeMap<i32, CancellationToken>
}
impl RemoteResources {
    fn new() -> Self {
        let ghclient = RemoteProjectHub::new();
        let persistent = Persistent::new();

        RemoteResources {
            repo: ghclient.repository("Pctg-x8/peridot"),
            ghclient, persistent
        }
    }
}
impl SlackBotLogic for BotLogic {
    fn launch(api: &AsyncSlackApiSender, botinfo: &ConnectionAccountInfo, _teaminfo: &TeamInfo) -> Self {
        println!("Bot Connected as {}({})", botinfo.name, botinfo.id);

        let remote = Arc::new(RemoteResources::new());
        let iotimes = remote.persistent.forall_user_iotimes();
        let current_time = chrono::Utc::now().naive_utc();
        println!("CurrentTime: {}", current_time);
        let sched = Scheduler::new(UserNotifyHandler {
            remote: remote.clone(), api: api.clone()
        });
        let mut intime_cancellation_token = BTreeMap::new();
        let mut outtime_cancellation_token = BTreeMap::new();
        for iot in iotimes {
            let in_date = 
                if current_time.time() > iot.intime { current_time.date().succ() } else { current_time.date() };
            let out_date =
                if current_time.time() > iot.outtime { current_time.date().succ() } else { current_time.date() };
            let intime = in_date.and_time(iot.intime);
            let outtime = out_date.and_time(iot.outtime);
            println!("IOTime for {}: {} ~ {}", iot.slack_id, intime, outtime);
            intime_cancellation_token.insert(iot.github_id,
                sched.request(intime.timestamp() as _, UserNotify::Hello(iot.slack_id.clone(), iot.github_id, iot.github_login.clone())));
            outtime_cancellation_token.insert(iot.github_id,
                sched.request(outtime.timestamp() as _, UserNotify::ByeReport(iot.slack_id, iot.github_id, iot.github_login)));
        }
        sched.reset_timeout();

        BotLogic {
            remote, api: api.clone(), dparse: JpDepsParser::new(), bot_userid: botinfo.id.clone(),
            call_checker: CallingSentenceDetector::new(&botinfo.id), rewriter: Rewriter::new(vec![
                (r#"おは(よー|よっ|よん|よ([^う])|よ$)"#, |c| if let Some(f) = c.get(2) {
                    format!("おはよう{}", f.as_str()).into()
                }
                else { "おはよう".into() }),
                (r#"残っている|残りの"#, |_| "残ってる".into()),
                (r#"ございます(ー|っ)+|ござます"#, |_| "ございます".into()),
                (":pr:", |_| "PR".into()),
                ("なんかある", |_| "なにかある".into())
            ]),
            greeting_rx: Regex::new(r#"^([A-Za-z0-9\-_]+)で[\-~ー〜っ]*す[\-~ー〜っ！!。.]*"#).unwrap(),
            sched, intime_cancellation_token, outtime_cancellation_token
        }
    }
    fn on_message(&mut self, _: &AsyncSlackApiSender, e: MessageEvent) {
        if e.user == self.bot_userid { return; }
        if e.subtype == Some("bot_message") { return; }
        println!("Incoming: {} from <@{}> in {}", e.text, e.user, e.channel);
        let process_start = std::time::Instant::now();
        
        // calling check
        if let Some((mut pv, cb, rest)) = self.call_checker.check(&e.text) {
            println!("CallBy: {:?}", cb);
            println!("Preverb: {:?}", pv);
            let checker_time = process_start.elapsed();
            let rest_fmt = self.rewriter.rewrite(rest);
            let rewriter_time = process_start.elapsed() - checker_time;
            println!("書き替え適用後の残りのペイロード:{}", rest_fmt);
            println!("ペイロードをパース:");

            let mut rest: &str = &rest_fmt;
            if pv != Some(Preverb::FirstContact) {
                let (pv2, rest2) = self.call_checker.strip_preverbs_after(&rest);
                pv = pv2.or(pv); rest = rest2;
            }
            if pv == Some(Preverb::FirstContact) {
                if let Some(caps) = self.greeting_rx.captures(&rest) {
                    self.first_contact(&e, caps.get(1).unwrap().as_str());
                    return;
                }
            }
            
            let n0 = process_start.elapsed();
            let cabotree = self.dparse.parse(&rest);
            let (root_chunks, linked_chunks) = cabotree.build_reverse_deps_tree();
            let dparse_time = process_start.elapsed() - n0;
            dump_rdeptree(&cabotree, &root_chunks, &linked_chunks);

            let n0 = process_start.elapsed();
            let funcalls: Vec<_> = root_chunks.into_iter()
                .filter_map(|ci| match pv { /* Preverbによって変形する */
                    Some(Preverb::Tell) => CallingForm::Verb("教える", VerbModifier::default(),
                        JpCallingEnv::from(JpArguments::from(JpArgument::parse_chunk(&cabotree, ci, &linked_chunks)))).into(),
                    _ => CallingForm::parse(&cabotree, ci, &linked_chunks)
                })
                .collect();
            let callform_parse_time = process_start.elapsed() - n0;
            println!("ExecutionTime Summary:");
            println!("* PrecallChecker: {} ms", as_millis_fract(&checker_time));
            println!("* Rewriter:       {} ms", as_millis_fract(&rewriter_time));
            println!("* JpDepParse:     {} ms", as_millis_fract(&dparse_time));
            println!("* CallFormParse:  {} ms", as_millis_fract(&callform_parse_time));
            println!("callforms:");
            for f in &funcalls {
                f.pretty_print(&mut std::io::stdout());
                std::io::stdout().flush().unwrap();
            }
            if funcalls.is_empty() {
                reply_to(&self.api, &e,
                    rand::thread_rng().choose(&["なぁに？", "呼んだー？", "呼んだ？", "どうしたの？"]).unwrap());
            }
            else {
                for f in funcalls.iter().map(Command::encode) {
                    match f.map_err(ProcessError::RecognizationFailure).and_then(|x| self.execute(&e, x)) {
                        Err(ProcessError::RecognizationFailure(msg)) =>
                            reply_to(&self.api, &e, &format!("ごめんね、{}", msg)),
                        Err(ProcessError::UserUnidentifiable) => 
                            reply_to(&self.api, &e, "だれ......？"),
                        Err(ProcessError::UncountableObjective) =>
                            reply_to(&self.api, &e, "数えられないよ！？"),
                        Err(ProcessError::Network(ne)) =>
                            report_error(&self.api, &e, &format!("NetworkError: {:?}", ne)),
                        Ok(_) => ()
                    }
                }
            }
        }
    }
}
#[derive(Debug)] pub enum ProcessError {
    UserUnidentifiable, UncountableObjective, Network(rq::Error), RecognizationFailure(String)
}
impl From<rq::Error> for ProcessError { fn from(v: rq::Error) -> Self { ProcessError::Network(v) } }
type ProcessResult<T> = Result<T, ProcessError>;
impl BotLogic {
    fn first_contact(&mut self, sender: &MessageEvent, github_username: &str) {
        match self.remote.ghclient.user(github_username) {
            Err(err) => report_error(&self.api, sender,
                &format!("An internal error during asking to GitHub for user: {:?}", err)),
            Ok(GitHubResponse::Success(u)) => {
                self.remote.persistent.init_user(u.response().id as _, sender.user, &u.response().login);
                let &(pretext, posttext) = rand::thread_rng().choose(&[
                    ("はじめまして！", "...さん？"),
                    ("わぁー、", "さん？はじめまして！"),
                    ("", "さん！これからよろしくね。")
                ]).unwrap();
                reply_to(&self.api, sender,
                    &format!("{}{}({}){}", pretext, github_username, u.response().name, posttext));
            },
            Ok(GitHubResponse::Failed { ref message }) if message == "Not Found" =>
                reply_to(&self.api, sender, "誰......？"),
            Ok(GitHubResponse::Failed { ref message }) =>
                reply_to(&self.api, sender, &format!("GitHubResponse: {}", message))
        }
    }
    fn execute(&mut self, sender: &MessageEvent, cmd: Command) -> ProcessResult<()> {
        println!("CommandTree: {:?}", cmd);
        // self.reply_to(&e, &format!("{:?}", obj));
        match cmd {
            Command::Greeting(following_desumasu) => reply_to(&self.api, sender,
                &self.remote.greeting(User::Slack(&sender.user), following_desumasu)?),
            Command::Tell(None) => reply_to(&self.api, sender, "何について知りたいの？"),
            Command::Tell(Some(tw)) => {
                let resp: Cow<str> = match tw {
                    TellWhat::RandomNumber => format!("{}", rand::random::<i64>()).into(),
                    // 明日の/昨日のタスクを取得する必要性がわからないのでひとまず無視
                    TellWhat::Tasks(user_calls, _, _) => self.remote.report_tasks(&user_calls, User::Slack(&sender.user))?,
                    TellWhat::CountOf(tw) => self.remote.count_source(&*tw, User::Slack(&sender.user))
                        .map(|(count, user_calls, ask_for_remains)|
                            self.remote.report_count(count, user_calls, ask_for_remains).into())?,
                    TellWhat::Issues { unresolved: true, .. } => self.remote.report_unresolved_issues()?,
                    TellWhat::PullRequests { opened_assignee, unreviewed, user_calls } =>
                        self.remote.report_pullrequests(&user_calls, opened_assignee, unreviewed)?,
                    TellWhat::Issues { unresolved: false, .. } => "Issues(unresolved=false)?".into()
                };
                reply_to(&self.api, sender, &resp);
            },
            Command::EndWorking(_at, _dayoffset) =>
                reply_to(&self.api, sender, &self.remote.workend(User::Slack(&sender.user))?),
        }

        return Ok(());
    }
}
fn as_millis_fract(d: &std::time::Duration) -> f64 { d.as_secs() as f64 + d.subsec_nanos() as f64 / 1_000_000.0 }
fn dump_rdeptree(tree: &cabo::TreeRef, root_chunks: &[usize], linked_chunks: &BTreeMap<usize, Vec<usize>>) {
    println!("dump-rdeptree:");
    for (ci, chunk) in root_chunks.iter().map(|&ci| (ci, tree.chunk(ci).unwrap())) {
        let tokens_chain = chunk.tokens(tree)
            .map(|tok| format!("{}({})<{}>", tok.surface_normalized(), tok.base_form(), tok.features().take(3).collect::<Vec<_>>().join("/")))
            .collect::<Vec<_>>().join("-");
        println!("- {}", tokens_chain);
        if let Some(chunks) = linked_chunks.get(&ci) {
            for &ci in chunks { rec_print(tree, &linked_chunks, ci, 1); }
        }

        fn rec_print(tree: &cabo::TreeRef, linked_chunks: &BTreeMap<usize, Vec<usize>>, target_index: usize, depth: usize) {
            let c = tree.chunk(target_index).unwrap();
            let tokens_chain = c.tokens(tree)
                .map(|t| format!("{}({})<{}>", t.surface_normalized(), t.base_form(), t.features().take(2).collect::<Vec<_>>().join("/")))
                .collect::<Vec<_>>().join("-");
            let verbs: Vec<_> = c.tokens(tree).filter(|t| t.primary_part() == "動詞").map(|x| x.base_form()).collect();

            print!("{}", std::iter::repeat(' ').take(depth).collect::<String>());
            if !verbs.is_empty() { println!("- {} {{Verbs:{:?}}}", tokens_chain, verbs); }
            else { println!("- {}", tokens_chain); }

            if let Some(chunks) = linked_chunks.get(&target_index) {
                for &ci in chunks { rec_print(tree, linked_chunks, ci, depth + 1); }
            }
        }
    }
}

struct CurrentTaskState {
    number: usize, title: String, url: String, progress: (usize, usize), is_pr: bool,
    pipeline_stage_name: String
}

use std::fmt::{Display, Formatter, Result as FmtResult};
struct TaskAchievementWriter(CurrentTaskState);
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
struct TaskUnresolvedCountWriter(CurrentTaskState);
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

struct OpenedPullRequestFormatter<'p>(&'p PullRequestInfo, ExtraIssueInfo, usize, usize);
impl<'p> Display for OpenedPullRequestFormatter<'p> {
    fn fmt(&self, fmt: &mut Formatter) -> FmtResult {
        let stage_str =
            if self.1.pipeline.name == "In Progress" { format!("*{} ステージ*", self.1.pipeline.name) }
            else { format!("{} ステージ", self.1.pipeline.name) };
        let miscinfo =
            if self.3 == 0 { stage_str }
            else { format!("{}, {}個中{}個完了", stage_str, self.2, self.3) };
        
        write!(fmt, "<{}|#{}>: {} ({})", self.0.html_url, self.0.number, self.0.title, miscinfo)
    }
}
impl<'p> OpenedPullRequestFormatter<'p> {
    fn new(pr: &'p PullRequestInfo, repo: &RemoteRepository) -> Self {
        let einfo = repo.issue_on_board(pr.number).unwrap().unwrap();
        let unchecked_count = pr.body.matches("- [ ]").count();
        let checked_count = pr.body.matches("- [x]").count();
        let total_checks = unchecked_count + checked_count;

        OpenedPullRequestFormatter(pr, einfo, checked_count, total_checks)
    }
}

enum User<'s> { Slack(&'s str), GitHub(i32, Cow<'s, str>) }
impl<'s> User<'s> {
    #[cfg(feature = "offline-check")]
    fn identify_as_github_id(self, persistent: &Persistent) -> ProcessResult<(i32, Cow<'s, str>)> {
        match self {
            User::Slack(_) => Ok((1234567, Cow::Borrowed("GitHubMockUser"))),
            User::GitHub(id, login) => Ok((id, login))
        }
    }
    #[cfg(not(feature = "offline-check"))]
    fn identify_as_github_id(self, persistent: &Persistent) -> ProcessResult<(i32, Cow<'s, str>)> {
        match self {
            User::Slack(skid) => persistent.query_github_from_slack_id(skid).map(|(a, b)| (a, b.into()))
                .ok_or(ProcessError::UserUnidentifiable),
            User::GitHub(id, login) => Ok((id, login))
        }
    }
}

fn reply_to(api: &AsyncSlackApiSender, src: &MessageEvent, msg: &str) {
    api.send(&chat::PostMessageParams {
        channel: src.channel, text: &format!("<@{}> {}", src.user, msg), as_user: Some(true), .. Default::default()
    });
}
fn general_reply_to_id(api: &AsyncSlackApiSender, id: &str, msg: &str) {
    const RESPONSE_CHANNEL: &'static str = "CDQ0SLXG8";
    api.send(&chat::PostMessageParams {
        channel: RESPONSE_CHANNEL, text: &format!("<@{}> {}", id, msg), as_user: Some(true), .. Default::default()
    });
}
fn report_error(api: &AsyncSlackApiSender, replying: &MessageEvent, msg: &str) {
    api.send(&chat::PostMessageParams {
        channel: replying.channel, text: &format!("An error occured in replying to {:?}", replying.text), as_user: Some(true),
        attachments: vec![chat::Attachment {
            color: Some("danger".into()), text: Some(msg.into()), .. Default::default()
        }], .. Default::default()
    })
}

fn utcnow() -> chrono::NaiveDateTime { chrono::Utc::now().naive_utc() }

/*impl BotLogic {
    fn greeting_update(&mut self, slkid: String) {
        if let Some(ghid) = self.remote.persistent.query_github_id_from_slack_id(&slkid) {
            self.sched.reset_timeout();
            let new_tk = self.sched.request(chrono::Utc::now().naive_local().timestamp() as _, UserNotify::Hello(slkid, ghid));
            let old_tok = std::mem::replace(self.intime_cancellation_token.get_mut(&ghid).unwrap(), new_tk);
            self.sched.cancel(old_tok);
        }
    }
}*/
impl RemoteResources {
    #[cfg(not(feature = "offline-check"))]
    fn collect_remaining_tasks(&self, user: User) -> ProcessResult<Vec<CurrentTaskState>> {
        let (_, ghlogin) = user.identify_as_github_id(&self.persistent)?;
        self.repo.opened_issues_by_assignee(&ghlogin)?.unwrap().into_iter().map(|s| {
            let exinfo = self.repo.issue_on_board(s.number)?.unwrap();
            let unchecked_count = s.body.matches("- [ ]").count();
            let checked_count = s.body.matches("- [x]").count();
            let total_checks = unchecked_count + checked_count;

            Ok(CurrentTaskState {
                number: s.number, title: s.title, url: s.html_url, progress: (checked_count, total_checks),
                is_pr: s.pull_request.is_some(), pipeline_stage_name: exinfo.pipeline.name
            })
        }).collect()
    }
    #[cfg(feature = "offline-check")]
    fn collect_remaining_tasks(&self, _: User) -> ProcessResult<Vec<CurrentTaskState>> {
        Ok(vec![CurrentTaskState {
            number: 1, title: "ダミータスク1".to_owned(), url: "http://github.com/Pctg-x8/peridot/issues/1".to_owned(),
            progress: (0, 1), is_pr: false, pipeline_stage_name: "In Progress".to_owned()
        }, CurrentTaskState {
            number: 2, title: "ダミープルリク1".to_owned(), url: "http://github.com/Pctg-x8/peridot/pulls/2".to_owned(),
            progress: (0, 0), is_pr: true, pipeline_stage_name: "Review/QA".to_owned()
        },])
    }

    fn greeting(&self, user: User, following_desumasu: bool) -> ProcessResult<String> {
        let (ghid, ghlogin) = user.identify_as_github_id(&self.persistent)?;
        if self.persistent.is_user_working(ghid) {
            self.greeting_message(None, following_desumasu)
        }
        else
        {
            let tasklist = self.collect_remaining_tasks(User::GitHub(ghid, ghlogin))?;
            self.persistent.user_setup_work(ghid, utcnow(), &tasklist.iter().map(|x| {
                let (a, b) = x.progress;
                RemainingWork { issue_id: x.number as _, progress: Some((a as _, b as _)) }
            }).collect::<Vec<_>>());
            return self.greeting_message(Some(tasklist), following_desumasu);
        }
    }
    fn workend(&self, user: User) -> ProcessResult<String> {
        let (ghid, ghlogin) = user.identify_as_github_id(&self.persistent)?;
        /*let user_remaining_tasks: Vec<_> = self.collect_remaining_tasks(user)?
            .into_iter().map(|x| TaskAchievementWriter(x).to_string()).collect();*/
        self.workend_message()
    }

    fn greeting_message(&self, tasklist: Option<Vec<CurrentTaskState>>, following_desumasu: bool) -> ProcessResult<String> {
        let &hello_text1 = if following_desumasu {
            rand::thread_rng().choose(&["おはようございます！", "おはようございまーす！"]).unwrap()
        }
        else {
            rand::thread_rng().choose(&["おはよう！", "おはよ！"]).unwrap()
        };
        if let Some(tasklist) = tasklist {
            let (hello_text2, tasklines, after_text);
            if tasklist.is_empty() {
                hello_text2 = *if following_desumasu {
                    rand::thread_rng().choose(&["今日のタスクは...0です！", "残ってるお仕事はありませんよー。"]).unwrap()
                }
                else {
                    rand::thread_rng().choose(&["今残ってるタスクはないかな。", "今日の献立は......えーっと......ないかも"]).unwrap()
                };
                tasklines = Vec::new();
                after_text = *if following_desumasu {
                    rand::thread_rng().choose(&[
                        "それじゃ、今日も張り切って行きましょー！！",
                        "毎日お疲れ様、今日も一日頑張って行きましょっ！"
                    ]).unwrap()
                }
                else {
                    rand::thread_rng().choose(&[
                        "それじゃ、今日もはりきっていこー！！",
                        "いつもお疲れ様。今日も頑張ってね！！",
                        "毎日頑張ってるね！今日も無理せずいこうね！！"
                    ]).unwrap()
                };
            }
            else {
                hello_text2 = *if following_desumasu {
                    rand::thread_rng().choose(&["今日のタスクはこれですよー！", "残りのお仕事はこちらでーす。"]).unwrap()
                }
                else {
                    rand::thread_rng().choose(&["今残ってるタスクはこれだよ。", "今日の献立だよー:issue-o::pr::issue-o::pr:"]).unwrap()
                };
                tasklines = tasklist.into_iter().map(|x| TaskAchievementWriter(x).to_string()).collect();
                after_text = *if following_desumasu {
                    rand::thread_rng().choose(&[
                        "それじゃ、今日も張り切って行きましょー！！",
                        "毎日お疲れ様、今日も一日頑張って行きましょっ！",
                        "まだまだ課題は山積みですよ！でもほどほどにこなしていきましょうね！"
                    ]).unwrap()
                }
                else {
                    rand::thread_rng().choose(&[
                        "それじゃ、今日もはりきっていこー！！",
                        "いつもお疲れ様。今日も頑張ってね！！",
                        "毎日頑張ってるね！今日も無理せずいこうね！！"
                    ]).unwrap()
                };
            }

            Ok(format!("{}{}\n{}\n{}", hello_text1, hello_text2, tasklines.join("\n"), after_text))
        }
        else {
            Ok(hello_text1.to_owned())
        }
    }
    fn workend_message(&self) -> ProcessResult<String> {
        let begin_text = rand::thread_rng().choose(&["今日もお疲れさま！"]).unwrap();
        let nodata_text = "";

        Ok(format!("{}{}", begin_text, nodata_text))
    }
    fn count_source<'s>(&self, what: &'s TellWhat, user: User) -> ProcessResult<(usize, &'s str, bool)> {
        match what {
            TellWhat::Issues { unresolved: true, user_calls, ask_for_remains } =>
                Ok((self.repo.opened_issues_by_labeled("UnresolvedQuestions")?.unwrap().len(), user_calls as _, *ask_for_remains)),
            TellWhat::Tasks(user_calls, afr, _) =>
                Ok((self.collect_remaining_tasks(user)?.len(), user_calls as _, *afr)),
            _ => Err(ProcessError::UncountableObjective)
        }
    }
    fn report_count(&self, count: usize, user_calls: &str, ask_for_remains: bool) -> String {
        format!("{}は{}{}個だよー", user_calls, if ask_for_remains { "あと" } else { "" }, count)
    }
    fn report_tasks(&self, user_calls: &str, user: User) -> ProcessResult<Cow<str>> {
        let tasklines: Vec<_> = self.collect_remaining_tasks(user)?.into_iter().map(TaskAchievementWriter)
            .map(|x| x.to_string()).collect();
        
        Ok(format!("{}は以下の通りだよ:\n{}", user_calls, tasklines.join("\n")).into())
    }
    fn report_pullrequests(&self, user_calls: &str, opened_assignee: bool, unreviewed: bool)
            -> rq::Result<Cow<str>> {
        let prs = self.repo.pullrequests()?.unwrap();
        if !opened_assignee && !unreviewed {
            // random
            if let Some(pr) = rand::thread_rng().choose(&prs) {
                Ok(format!("はい。\n{}", OpenedPullRequestFormatter::new(pr, &self.repo)).into())
            }
            else {
                Ok(Cow::from(*rand::thread_rng().choose(&[
                    "開いてるPRはないみたい", "開いてるPRはないみたいだね。その調子！",
                    "うーん、ちょっと見当たらないかも......？", "今は届いてなさそうだね"
                ]).unwrap()))
            }
        }
        else {
            let filtered: Vec<_> = prs.iter().filter(|p| {
                (opened_assignee && p.assignees.iter().find(|a| a.login == "Pctg-x8").is_some()) ||
                (unreviewed && p.requested_reviewers.iter().find(|a| a.login == "Pctg-x8").is_some())
            }).map(|p| OpenedPullRequestFormatter::new(p, &self.repo)).collect();

            if filtered.is_empty() {
                let mut haystack = vec![
                    Cow::from(format!("{}はないみたい", user_calls)), format!("{}はないみたいだね。その調子！", user_calls).into(),
                    "うーん、ちょっと見当たらないかも......？".into(), "今は届いてなさそうだね".into()
                ];
                let index = rand::thread_rng().gen_range(0, haystack.len());

                Ok(haystack.remove(index))
            }
            else {
                let msg = format!("{}はこれかなー\n{}", user_calls,
                    filtered.into_iter().map(|s| s.to_string()).collect::<Vec<_>>().join("\n"));
                Ok(msg.into())
            }
        }
    }
    fn report_unresolved_issues(&self) -> rq::Result<Cow<str>> {
        let issues = self.repo.unresolved_issues()?.unwrap();
        if issues.is_empty() { Ok("未解決の課題はないかなぁ".into()) }
        else {
            let headtext = "まだ解決してないことがあるのは以下の課題だよー。忘れないようにね！";
            let issuelines = issues.into_iter().map(|s| {
                let exinfo = self.repo.issue_on_board(s.number).unwrap().unwrap();
                let unchecked_count = s.body.matches("- [ ]").count();
                let checked_count = s.body.matches("- [x]").count();
                let total_checks = unchecked_count + checked_count;

                TaskUnresolvedCountWriter(CurrentTaskState {
                    number: s.number, title: s.title, url: s.html_url, progress: (checked_count, total_checks),
                    is_pr: s.pull_request.is_some(), pipeline_stage_name: exinfo.pipeline.name
                }).to_string()
            }).collect::<Vec<_>>();

            Ok(format!("{}\n{}", headtext, issuelines.join("\n")).into())
        }
    }
}
