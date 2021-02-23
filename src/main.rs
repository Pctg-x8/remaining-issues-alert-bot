use asyncslackbot::*;
use regex::Regex;
use slack::IApiSender;
use std::{collections::BTreeMap, future::Future};

use log::*;

mod cabocha;
use cabocha as cabo;

mod core;
mod slack;

mod sched;
use sched::{CancellationToken, Scheduler};

#[cfg(feature = "offline-check")]
mod readline;
#[cfg(feature = "offline-check")]
use readline::Readline;
use std::io::Write;

use rand::seq::SliceRandom;

#[cfg(not(feature = "offline-check"))]
fn slack_api_token() -> String {
    std::env::var("SLACK_API_TOKEN").expect("SLACK_API_TOKEN not set")
}
#[cfg(not(feature = "offline-check"))]
fn slack_app_token() -> String {
    std::env::var("SLACK_APP_TOKEN").expect("SLACK_APP_TOKEN not set")
}

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL not set")
}
fn redis_url() -> String {
    std::env::var("REDIS_URL").expect("REDIS_URL not set")
}

#[derive(Debug)]
#[cfg(feature = "offline-check")]
pub struct SlackWebApi2 {
    endpoint: &'static str,
    paramdata: String,
}
#[derive(serde::Deserialize, Debug)]
#[cfg(feature = "offline-check")]
pub struct PostMessageParams<'s> {
    pub channel: &'s str,
    pub text: std::borrow::Cow<'s, str>,
    pub as_user: Option<bool>,
    pub icon_emoji: Option<&'s str>,
    pub icon_url: Option<&'s str>,
    pub username: Option<&'s str>,
    pub attachments: Vec<Attachment<'s>>,
}
#[derive(serde::Deserialize, Debug)]
#[cfg(feature = "offline-check")]
pub struct Attachment<'s> {
    pub color: Option<std::borrow::Cow<'s, str>>,
    pub text: Option<std::borrow::Cow<'s, str>>,
}

#[async_std::main]
async fn main() {
    println!("SlackBot[Remaining Issues Alerting]");
    if dotenv::from_filename(".conf.e").is_err() {
        warn!("Failed to load .conf.e file");
    }
    env_logger::init();

    #[cfg(not(feature = "offline-check"))]
    while slack_socket_mode_client::run(&slack_app_token(), &mut Logic::new().await)
        .await
        .expect("failure in socket mode loop")
        == slack_socket_mode_client::DisconnectReason::RefreshRequested
    {}

    /*
    #[cfg(not(feature = "offline-check"))]
    launch_rtm::<BotLogic>(String::from(slack_api_token())).await;
    #[cfg(feature = "offline-check")]
    mock_rtm::<BotLogic>();
    */
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
    let sender = MockedSlackApiSender;
    let mut logic = L::launch(
        sender,
        &ConnectionAccountInfo {
            id: "UBOTDUMMY".to_owned(),
            name: "koyuki".to_owned(),
        },
        &TeamInfo {
            id: "TRTMEMU".to_owned(),
            name: "RtmOffliceEmu".to_owned(),
            domain: "localhost".to_owned(),
        },
    );
    let rl = Readline::init(None);
    while let Some(line) = rl.readline("> ") {
        logic.on_message(MessageEvent {
            text: std::borrow::Cow::Borrowed(line.as_str().unwrap()),
            channel: "DDUMMYCH",
            ts: "123456789",
            user: "UDUMMYSENDER",
            subtype: None,
        });
    }
}

use std::sync::Arc;

pub struct UserNotifyHandler {
    remote: Arc<core::RemoteResources>,
    api: Arc<slack::ApiClient>,
}
impl sched::EventHandler<UserNotify> for UserNotifyHandler {
    fn handle_batch(&self, events: Vec<Option<UserNotify>>) {
        for e in events.into_iter().filter_map(|x| x) {
            println!("Notify: {:?}", e);
            async_std::task::block_on(async {
                match e {
                    UserNotify::Hello(skid, ghid, ghl) => match self
                        .remote
                        .greeting(core::User::GitHub(ghid, ghl.into()), false)
                        .await
                    {
                        Ok(m) => unsafe {
                            general_reply_to_id(&self.api, &skid, &m).await;
                        },
                        Err(er) => {
                            error!("An error occured while generating hello message: {:?}", er);
                        }
                    },
                    UserNotify::ByeReport(skid, ghid, ghl) => {
                        match self.remote.workend(core::User::GitHub(ghid, ghl.into())) {
                            Ok(m) => unsafe {
                                general_reply_to_id(&self.api, &skid, &m).await;
                            },
                            Err(er) => {
                                error!(
                                    "An error occured while generating workend message: {:?}",
                                    er
                                );
                            }
                        }
                    }
                }
            });
        }
    }
}

struct Logic {
    userid: String,
    call_checker: core::CallingSentenceDetector,
    rewriter: core::Rewriter<'static>,
    greeting_rx: Regex,
    dparse: core::JpDepsParser,
    remote: Arc<core::RemoteResources>,
    api_client: Arc<slack::ApiClient>,
    sched: Scheduler<UserNotify, UserNotifyHandler>,
    intime_cancellation_token: BTreeMap<i32, CancellationToken>,
    outtime_cancellation_token: BTreeMap<i32, CancellationToken>,
}
impl Logic {
    async fn new() -> Self {
        let api_client = Arc::new(slack::ApiClient::new(&slack_api_token()));
        let auth_test = api_client
            .send2(&slack::AuthTestRequest)
            .await
            .expect("Failed to call api")
            .into_result()
            .expect("auth.test failed");
        let remote = Arc::new(core::RemoteResources::new(&database_url(), &redis_url()).await);

        let iotimes = remote.persistent.forall_user_iotimes();
        let current_time = chrono::Utc::now().naive_utc();
        info!("CurrentTime: {}", current_time);
        let sched = Scheduler::new(UserNotifyHandler {
            remote: remote.clone(),
            api: api_client.clone(),
        }).await;
        let mut intime_cancellation_token = BTreeMap::new();
        let mut outtime_cancellation_token = BTreeMap::new();
        for iot in iotimes {
            let in_date = if current_time.time() > iot.intime {
                current_time.date().succ()
            } else {
                current_time.date()
            };
            let out_date = if current_time.time() > iot.outtime {
                current_time.date().succ()
            } else {
                current_time.date()
            };
            let intime = in_date.and_time(iot.intime);
            let outtime = out_date.and_time(iot.outtime);
            println!("IOTime for {}: {} ~ {}", iot.slack_id, intime, outtime);
            intime_cancellation_token.insert(
                iot.github_id,
                sched.request(
                    intime.timestamp() as _,
                    UserNotify::Hello(
                        iot.slack_id.clone(),
                        iot.github_id,
                        iot.github_login.clone(),
                    ),
                ),
            );
            outtime_cancellation_token.insert(
                iot.github_id,
                sched.request(
                    outtime.timestamp() as _,
                    UserNotify::ByeReport(iot.slack_id, iot.github_id, iot.github_login),
                ),
            );
        }
        sched.reset_timeout().await;

        Self {
            call_checker: core::CallingSentenceDetector::new(&auth_test.user_id),
            userid: auth_test.user_id,
            rewriter: core::Rewriter::new(vec![
                (r#"おは(よー|よっ|よん|よ([^う])|よ$)"#, |c| {
                    if let Some(f) = c.get(2) {
                        format!("おはよう{}", f.as_str()).into()
                    } else {
                        "おはよう".into()
                    }
                }),
                (r#"残っている|残りの"#, |_| "残ってる".into()),
                (r#"ございます(ー|っ)+|ござます"#, |_| {
                    "ございます".into()
                }),
                (":pr:", |_| "PR".into()),
                ("なんかある", |_| "なにかある".into()),
            ]),
            greeting_rx: Regex::new(r#"^([A-Za-z0-9\-_]+)で[\-~ー〜っ]*す[\-~ー〜っ！!。.]*"#)
                .unwrap(),
            dparse: core::JpDepsParser::new(),
            remote,
            api_client,
            sched,
            intime_cancellation_token,
            outtime_cancellation_token,
        }
    }
}
impl slack_socket_mode_client::EventHandler for Logic {
    fn on_events_api(&mut self, payload: slack_socket_mode_client::protocol::EventsApiPayload) {
        info!("payload: {:?}", payload);
        match payload.event {
            slack_socket_mode_client::protocol::events_api::Event::Message(msg) => {
                if self.is_respondable_message(&msg) {
                    self.respond_message(msg);
                }
            }
            _ => (/* nop */),
        }
    }
}
impl Logic {
    fn is_respondable_message(
        &self,
        message: &slack_socket_mode_client::protocol::events_api::MessageEvent,
    ) -> bool {
        if message.user.as_ref().map_or(true, |&i| i == self.userid) {
            return false;
        }
        if message.subtype.is_some() {
            return false;
        }
        true
    }
    fn respond_message(
        &mut self,
        message: slack_socket_mode_client::protocol::events_api::MessageEvent,
    ) {
        println!(
            "Incoming: {:?} from <@{:?}> in {:?}",
            message.text, message.user, message.channel
        );

        let process_start = std::time::Instant::now();
        let text = match message.text {
            Some(ref t) => t,
            None => return,
        };

        // calling check
        let (mut pv, cb, rest) = match self.call_checker.check(&text) {
            Some(p) => p,
            None => return,
        };
        println!("CallBy: {:?}", cb);
        println!("Preverb: {:?}", pv);
        let checker_time = process_start.elapsed();
        let rest_fmt = self.rewriter.rewrite(rest);
        let rewriter_time = process_start.elapsed() - checker_time;
        trace!("書き替え適用後の残りのペイロード:{}", rest_fmt);
        trace!("Begin parse payload...");

        let mut rest: &str = &rest_fmt;
        if pv != Some(core::Preverb::FirstContact) {
            let (pv2, rest2) = self.call_checker.strip_preverbs_after(&rest);
            pv = pv2.or(pv);
            rest = rest2;
        }
        if pv == Some(core::Preverb::FirstContact) {
            if let Some(caps) = self.greeting_rx.captures(&rest) {
                let gh_username = String::from(caps.get(1).unwrap().as_str());
                let exec = core::CommandExecutor::new(&self.remote);
                let reply_to_user = ReplyUserIdentity::from(&message);
                let text = String::from(text as &str);
                let api_client = self.api_client.clone();

                async_std::task::spawn(async move {
                    match exec.first_contact(&gh_username, &reply_to_user.user).await {
                        Ok(s) => {
                            if let Err(e) = reply_to(&api_client, &reply_to_user, &s).await {
                                report_error(
                                    &api_client,
                                    &reply_to_user,
                                    &text,
                                    &format!(
                                        "An error occured while posting first content reply: {:?}",
                                        e
                                    ),
                                )
                                .await
                                .expect("Failed to post error message");
                            }
                        }
                        Err(err) => {
                            report_error(&api_client, &reply_to_user, &text, &format!("{:?}", err))
                                .await
                                .expect("Failed to post error message");
                        }
                    }
                });
                return;
            }
        }

        let n0 = process_start.elapsed();
        let cabotree = self.dparse.parse(&rest);
        let (root_chunks, linked_chunks) = cabotree.build_reverse_deps_tree();
        let dparse_time = process_start.elapsed() - n0;
        dump_rdeptree(&cabotree, &root_chunks, &linked_chunks);

        let n0 = process_start.elapsed();
        let funcalls: Vec<_> = root_chunks
            .into_iter()
            .filter_map(|ci| match pv {
                /* Preverbによって変形する */
                Some(core::Preverb::Tell) => core::CallingForm::Verb(
                    "教える",
                    core::VerbModifier::default(),
                    core::JpCallingEnv::from(core::JpArguments::from(
                        core::JpArgument::parse_chunk(&cabotree, ci, &linked_chunks),
                    )),
                )
                .into(),
                _ => core::CallingForm::parse(&cabotree, ci, &linked_chunks),
            })
            .collect();
        let callform_parse_time = process_start.elapsed() - n0;
        debug!("ExecutionTime Summary:");
        debug!("* PrecallChecker: {:?}", checker_time);
        debug!("* Rewriter:       {:?}", rewriter_time);
        debug!("* JpDepParse:     {:?}", dparse_time);
        debug!("* CallFormParse:  {:?}", callform_parse_time);
        println!("callforms:");
        for f in &funcalls {
            f.pretty_print(&mut std::io::stdout());
            std::io::stdout().flush().unwrap();
        }
        if funcalls.is_empty() {
            let msg = ["なぁに？", "呼んだー？", "呼んだ？", "どうしたの？"]
                .choose(&mut rand::thread_rng())
                .unwrap();
            async_std::task::spawn(reply_to(
                &self.api_client,
                &ReplyUserIdentity::from(&message),
                msg,
            ));
        } else {
            for f in funcalls.iter().map(core::Command::encode) {
                let exec = core::CommandExecutor::new(&self.remote);
                let api_client = self.api_client.clone();
                let reply_to_user = ReplyUserIdentity::from(&message);
                let text = String::from(text as &str);

                async_std::task::spawn(async move {
                    let res = match f {
                        Ok(x) => match exec
                            .execute(x, core::User::Slack(&reply_to_user.user))
                            .await
                        {
                            Ok(s) => {
                                if let Err(e) = reply_to(&api_client, &reply_to_user, &s).await {
                                    report_error(&api_client, &reply_to_user, &text, &e)
                                        .await
                                        .expect("Failed to post error message");
                                }
                                Ok(())
                            }
                            Err(e) => Err(e),
                        },
                        Err(e) => Err(core::ProcessError::RecognizationFailure(e)),
                    };

                    if let Err(e) = res {
                        let r = match e {
                            core::ProcessError::RecognizationFailure(msg) => {
                                reply_to(&api_client, &reply_to_user, &format!("ごめんね、{}", msg))
                                    .await
                                    .expect("Failed to respond as RecognizationFailure")
                            }
                            core::ProcessError::UserUnidentifiable => {
                                debug!("userunidentifiable: {:?}", reply_to_user);
                                reply_to(&api_client, &reply_to_user, "だれ......？")
                                    .await
                                    .expect("Failed to respond as UserUnidentifiable")
                            }
                            core::ProcessError::UncountableObjective => {
                                reply_to(&api_client, &reply_to_user, "数えられないよ！？")
                                    .await
                                    .expect("Failed to respond as UncountableObjective")
                            }
                            core::ProcessError::Network(ne) => report_error(
                                &api_client,
                                &reply_to_user,
                                &text,
                                &format!("NetworkError: {:?}", ne),
                            )
                            .await
                            .expect("Failed to respond as Network Error"),
                            core::ProcessError::GithubAPIError(e) => report_error(
                                &api_client,
                                &reply_to_user,
                                &text,
                                &format!("Network Error while requesting to github api: {:?}", e),
                            )
                            .await
                            .expect("Failed to respond as GithubApiError"),
                        };
                        if let slack::ApiResult::Errored(e) = r {
                            error!("post apicall failed: {}", e);
                        }
                    }
                });
            }
        }
    }
}

#[derive(Debug)]
pub enum UserNotify {
    Hello(String, i32, String),
    ByeReport(String, i32, String),
}
/*
pub struct BotLogic {
    remote: Arc<core::RemoteResources>,
    bot_userid: String,
    api_client: Arc<slack::ApiClient>,
    call_checker: core::CallingSentenceDetector,
    rewriter: core::Rewriter<'static>,
    dparse: core::JpDepsParser,
    greeting_rx: Regex,
    sched: Scheduler<UserNotify, UserNotifyHandler>,
    intime_cancellation_token: BTreeMap<i32, CancellationToken>,
    outtime_cancellation_token: BTreeMap<i32, CancellationToken>,
}
impl SlackBotLogic for BotLogic {
    fn launch(
        api: AsyncSlackApiSender,
        botinfo: &ConnectionAccountInfo,
        _teaminfo: &TeamInfo,
    ) -> Self {
        println!("Bot Connected as {}({})", botinfo.name, botinfo.id);
        let api_client = Arc::new(slack::ApiClient::new(&slack_api_token()));

        let remote = Arc::new(async_std::task::block_on(core::RemoteResources::new(
            &database_url(),
            &redis_url(),
        )));
        let iotimes = remote.persistent.forall_user_iotimes();
        let current_time = chrono::Utc::now().naive_utc();
        println!("CurrentTime: {}", current_time);
        let sched = Scheduler::new(UserNotifyHandler {
            remote: remote.clone(),
            api: api_client.clone(),
        });
        let mut intime_cancellation_token = BTreeMap::new();
        let mut outtime_cancellation_token = BTreeMap::new();
        for iot in iotimes {
            let in_date = if current_time.time() > iot.intime {
                current_time.date().succ()
            } else {
                current_time.date()
            };
            let out_date = if current_time.time() > iot.outtime {
                current_time.date().succ()
            } else {
                current_time.date()
            };
            let intime = in_date.and_time(iot.intime);
            let outtime = out_date.and_time(iot.outtime);
            println!("IOTime for {}: {} ~ {}", iot.slack_id, intime, outtime);
            intime_cancellation_token.insert(
                iot.github_id,
                sched.request(
                    intime.timestamp() as _,
                    UserNotify::Hello(
                        iot.slack_id.clone(),
                        iot.github_id,
                        iot.github_login.clone(),
                    ),
                ),
            );
            outtime_cancellation_token.insert(
                iot.github_id,
                sched.request(
                    outtime.timestamp() as _,
                    UserNotify::ByeReport(iot.slack_id, iot.github_id, iot.github_login),
                ),
            );
        }
        sched.reset_timeout();

        BotLogic {
            remote,
            dparse: core::JpDepsParser::new(),
            bot_userid: botinfo.id.clone(),
            call_checker: core::CallingSentenceDetector::new(&botinfo.id),
            rewriter: core::Rewriter::new(vec![
                (r#"おは(よー|よっ|よん|よ([^う])|よ$)"#, |c| {
                    if let Some(f) = c.get(2) {
                        format!("おはよう{}", f.as_str()).into()
                    } else {
                        "おはよう".into()
                    }
                }),
                (r#"残っている|残りの"#, |_| "残ってる".into()),
                (r#"ございます(ー|っ)+|ござます"#, |_| {
                    "ございます".into()
                }),
                (":pr:", |_| "PR".into()),
                ("なんかある", |_| "なにかある".into()),
            ]),
            greeting_rx: Regex::new(r#"^([A-Za-z0-9\-_]+)で[\-~ー〜っ]*す[\-~ー〜っ！!。.]*"#)
                .unwrap(),
            sched,
            intime_cancellation_token,
            outtime_cancellation_token,
            api_client,
        }
    }
    fn on_message(&mut self, e: MessageEvent) {
        if e.user == self.bot_userid {
            return;
        }
        if e.subtype == Some("bot_message") {
            return;
        }
        println!("Incoming: {} from <@{}> in {}", e.text, e.user, e.channel);
        let process_start = std::time::Instant::now();

        // calling check
        if let Some((mut pv, cb, rest)) = self.call_checker.check(&e.text) {
            println!("CallBy: {:?}", cb);
            println!("Preverb: {:?}", pv);
            let checker_time = process_start.elapsed();
            let rest_fmt = self.rewriter.rewrite(rest);
            let rewriter_time = process_start.elapsed() - checker_time;
            trace!("書き替え適用後の残りのペイロード:{}", rest_fmt);
            trace!("ペイロードをパース:");

            let mut rest: &str = &rest_fmt;
            if pv != Some(core::Preverb::FirstContact) {
                let (pv2, rest2) = self.call_checker.strip_preverbs_after(&rest);
                pv = pv2.or(pv);
                rest = rest2;
            }
            if pv == Some(core::Preverb::FirstContact) {
                if let Some(caps) = self.greeting_rx.captures(&rest) {
                    let gh_username = String::from(caps.get(1).unwrap().as_str());
                    let exec = core::CommandExecutor::new(&self.remote);
                    let reply_to_user = ReplyUserIdentity::from(&e);
                    let src_text = e.text.into_owned();
                    let api_client = self.api_client.clone();

                    async_std::task::spawn(async move {
                        match exec.first_contact(&gh_username, &reply_to_user.user).await {
                            Ok(s) => {
                                reply_to(&api_client, &reply_to_user, &s).await;
                            }
                            Err(err) => {
                                report_error(
                                    &api_client,
                                    &reply_to_user,
                                    &src_text,
                                    &format!("{:?}", err),
                                )
                                .await;
                            }
                        }
                    });
                    return;
                }
            }

            let n0 = process_start.elapsed();
            let cabotree = self.dparse.parse(&rest);
            let (root_chunks, linked_chunks) = cabotree.build_reverse_deps_tree();
            let dparse_time = process_start.elapsed() - n0;
            dump_rdeptree(&cabotree, &root_chunks, &linked_chunks);

            let n0 = process_start.elapsed();
            let funcalls: Vec<_> = root_chunks
                .into_iter()
                .filter_map(|ci| match pv {
                    /* Preverbによって変形する */
                    Some(core::Preverb::Tell) => core::CallingForm::Verb(
                        "教える",
                        core::VerbModifier::default(),
                        core::JpCallingEnv::from(core::JpArguments::from(
                            core::JpArgument::parse_chunk(&cabotree, ci, &linked_chunks),
                        )),
                    )
                    .into(),
                    _ => core::CallingForm::parse(&cabotree, ci, &linked_chunks),
                })
                .collect();
            let callform_parse_time = process_start.elapsed() - n0;
            debug!("ExecutionTime Summary:");
            debug!("* PrecallChecker: {} ms", as_millis_fract(&checker_time));
            debug!("* Rewriter:       {} ms", as_millis_fract(&rewriter_time));
            debug!("* JpDepParse:     {} ms", as_millis_fract(&dparse_time));
            debug!(
                "* CallFormParse:  {} ms",
                as_millis_fract(&callform_parse_time)
            );
            println!("callforms:");
            for f in &funcalls {
                f.pretty_print(&mut std::io::stdout());
                std::io::stdout().flush().unwrap();
            }
            if funcalls.is_empty() {
                let msg = ["なぁに？", "呼んだー？", "呼んだ？", "どうしたの？"]
                    .choose(&mut rand::thread_rng())
                    .unwrap();
                async_std::task::spawn(reply_to(&self.api_client, &From::from(&e), msg));
            } else {
                for f in funcalls.iter().map(core::Command::encode) {
                    let exec = core::CommandExecutor::new(&self.remote);
                    let api_client = self.api_client.clone();
                    let reply_to_user = ReplyUserIdentity::from(&e);
                    let src_text = String::from(&e.text as &str);

                    async_std::task::spawn(async move {
                        let res = match f {
                            Ok(x) => match exec
                                .execute(x, core::User::Slack(&reply_to_user.user))
                                .await
                            {
                                Ok(s) => {
                                    reply_to(&api_client, &reply_to_user, &s).await;
                                    Ok(())
                                }
                                Err(e) => Err(e),
                            },
                            Err(e) => Err(core::ProcessError::RecognizationFailure(e)),
                        };

                        if let Err(e) = res {
                            match e {
                                core::ProcessError::RecognizationFailure(msg) => {
                                    reply_to(
                                        &api_client,
                                        &reply_to_user,
                                        &format!("ごめんね、{}", msg),
                                    )
                                    .await;
                                }
                                core::ProcessError::UserUnidentifiable => {
                                    reply_to(&api_client, &reply_to_user, "だれ......？").await;
                                }
                                core::ProcessError::UncountableObjective => {
                                    reply_to(&api_client, &reply_to_user, "数えられないよ！？")
                                        .await;
                                }
                                core::ProcessError::Network(ne) => {
                                    report_error(
                                        &api_client,
                                        &reply_to_user,
                                        &src_text,
                                        &format!("NetworkError: {:?}", ne),
                                    )
                                    .await;
                                }
                                core::ProcessError::GithubAPIError(e) => {
                                    report_error(
                                        &api_client,
                                        &reply_to_user,
                                        &src_text,
                                        &format!(
                                            "Network Error while requesting to github api: {:?}",
                                            e
                                        ),
                                    )
                                    .await;
                                }
                            }
                        }
                    });
                }
            }
        }
    }
}
*/

fn dump_rdeptree(
    tree: &cabo::TreeRef,
    root_chunks: &[usize],
    linked_chunks: &BTreeMap<usize, Vec<usize>>,
) {
    println!("dump-rdeptree:");
    for (ci, chunk) in root_chunks.iter().map(|&ci| (ci, tree.chunk(ci).unwrap())) {
        let tokens_chain = chunk
            .tokens(tree)
            .map(|tok| {
                format!(
                    "{}({})<{}>",
                    tok.surface_normalized(),
                    tok.base_form(),
                    tok.features().take(3).collect::<Vec<_>>().join("/")
                )
            })
            .collect::<Vec<_>>()
            .join("-");
        println!("- {}", tokens_chain);
        if let Some(chunks) = linked_chunks.get(&ci) {
            for &ci in chunks {
                rec_print(tree, &linked_chunks, ci, 1);
            }
        }

        fn rec_print(
            tree: &cabo::TreeRef,
            linked_chunks: &BTreeMap<usize, Vec<usize>>,
            target_index: usize,
            depth: usize,
        ) {
            let c = tree.chunk(target_index).unwrap();
            let tokens_chain = c
                .tokens(tree)
                .map(|t| {
                    format!(
                        "{}({})<{}>",
                        t.surface_normalized(),
                        t.base_form(),
                        t.features().take(2).collect::<Vec<_>>().join("/")
                    )
                })
                .collect::<Vec<_>>()
                .join("-");
            let verbs: Vec<_> = c
                .tokens(tree)
                .filter(|t| t.primary_part() == "動詞")
                .map(|x| x.base_form())
                .collect();

            print!("{}", std::iter::repeat(' ').take(depth).collect::<String>());
            if !verbs.is_empty() {
                println!("- {} {{Verbs:{:?}}}", tokens_chain, verbs);
            } else {
                println!("- {}", tokens_chain);
            }

            if let Some(chunks) = linked_chunks.get(&target_index) {
                for &ci in chunks {
                    rec_print(tree, linked_chunks, ci, depth + 1);
                }
            }
        }
    }
}

#[derive(Debug)]
struct ReplyUserIdentity {
    channel: String,
    user: String,
}
impl<'m> From<&'m MessageEvent<'m>> for ReplyUserIdentity {
    fn from(v: &MessageEvent) -> Self {
        ReplyUserIdentity {
            channel: String::from(v.channel),
            user: String::from(v.user),
        }
    }
}
impl<'m> From<&'m slack_socket_mode_client::protocol::events_api::MessageEvent<'m>>
    for ReplyUserIdentity
{
    fn from(v: &slack_socket_mode_client::protocol::events_api::MessageEvent) -> Self {
        ReplyUserIdentity {
            channel: String::from(v.channel),
            user: String::from(v.user.expect("replying to empty user?")),
        }
    }
}

fn reply_to<A: slack::IApiSender>(
    api: &A,
    src: &ReplyUserIdentity,
    msg: &str,
) -> impl Future<Output = Result<slack::ApiResult<()>, A::Error>> {
    api.send(&chat::PostMessageParams {
        channel: &src.channel,
        text: &format!("<@{}> {}", &src.user, msg),
        as_user: Some(true),
        ..Default::default()
    })
}
fn general_reply_to_id<A: slack::IApiSender>(
    api: &A,
    id: &str,
    msg: &str,
) -> impl Future<Output = Result<slack::ApiResult<()>, A::Error>> {
    const RESPONSE_CHANNEL: &'static str = "CDQ0SLXG8";
    api.send(&chat::PostMessageParams {
        channel: RESPONSE_CHANNEL,
        text: &format!("<@{}> {}", id, msg),
        as_user: Some(true),
        ..Default::default()
    })
}
fn report_error<A: slack::IApiSender, E: std::fmt::Debug>(
    api: &A,
    replying: &ReplyUserIdentity,
    text: &str,
    error: &E,
) -> impl Future<Output = Result<slack::ApiResult<()>, A::Error>> {
    api.send(&chat::PostMessageParams {
        channel: &replying.channel,
        text: &format!("An error occured in replying to {:?}", text),
        as_user: Some(true),
        attachments: vec![chat::Attachment {
            color: Some("danger".into()),
            text: Some(std::borrow::Cow::Owned(format!("{:?}", error))),
            ..Default::default()
        }],
        ..Default::default()
    })
}

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
