
use asyncslackbot::*;
use regex::Regex;
use std::collections::BTreeMap;

use log::*;

mod cabocha; use cabocha as cabo;

mod slack;
mod core;

mod sched; use sched::{Scheduler, CancellationToken};

#[cfg(feature = "offline-check")]
mod readline;
#[cfg(feature = "offline-check")]
use readline::Readline;
use std::io::Write;

use rand::seq::SliceRandom;

#[cfg(not(feature = "offline-check"))]
fn slack_api_token() -> String { std::env::var("SLACK_API_TOKEN").expect("SLACK_API_TOKEN not set") }

fn database_url() -> String { std::env::var("DATABASE_URL").expect("DATABASE_URL not set") }
fn redis_url() -> String { std::env::var("REDIS_URL").expect("REDIS_URL not set") }

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

#[async_std::main]
async fn main() {
    println!("SlackBot[Remaining Issues Alerting]");
    if dotenv::from_filename(".conf.e").is_err() { warn!("Failed to load .conf.e file"); }

    #[cfg(not(feature = "offline-check"))] launch_rtm::<BotLogic>(String::from(slack_api_token())).await;
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

use std::sync::Arc;

pub struct UserNotifyHandler { remote: Arc<core::RemoteResources>, api: Arc<AsyncSlackApiSender> }
impl sched::EventHandler<UserNotify> for UserNotifyHandler {
    fn handle_batch(&self, events: Vec<Option<UserNotify>>) {
        for e in events.into_iter().filter_map(|x| x) {
            println!("Notify: {:?}", e);
            async_std::task::block_on(async {
                match e {
                    UserNotify::Hello(skid, ghid, ghl) =>
                        match self.remote.greeting(core::User::GitHub(ghid, ghl.into()), false).await {
                            Ok(m) => unsafe { general_reply_to_id(&self.api, &skid, &m).await },
                            Err(er) => { eprintln!("An error occured while generating hello message: {:?}", er); }
                        },
                    UserNotify::ByeReport(skid, ghid, ghl) =>
                        match self.remote.workend(core::User::GitHub(ghid, ghl.into())) {
                            Ok(m) => unsafe { general_reply_to_id(&self.api, &skid, &m).await; },
                            Err(er) => { eprintln!("An error occured while generating workend message: {:?}", er); }
                        }
                }
            });
        }
    }
}

#[derive(Debug)]
pub enum UserNotify { Hello(String, i32, String), ByeReport(String, i32, String) }
pub struct BotLogic {
    remote: Arc<core::RemoteResources>, api: Arc<AsyncSlackApiSender>, bot_userid: String,
    call_checker: core::CallingSentenceDetector, rewriter: core::Rewriter<'static>,
    dparse: core::JpDepsParser, greeting_rx: Regex,
    sched: Scheduler<UserNotify, UserNotifyHandler>,
    intime_cancellation_token: BTreeMap<i32, CancellationToken>,
    outtime_cancellation_token: BTreeMap<i32, CancellationToken>
}
impl SlackBotLogic for BotLogic {
    fn launch(api: AsyncSlackApiSender, botinfo: &ConnectionAccountInfo, _teaminfo: &TeamInfo) -> Self {
        println!("Bot Connected as {}({})", botinfo.name, botinfo.id);
        let apiclient = Arc::new(api);

        let remote = Arc::new(async_std::task::block_on(core::RemoteResources::new(&database_url(), &redis_url())));
        let iotimes = remote.persistent.forall_user_iotimes();
        let current_time = chrono::Utc::now().naive_utc();
        println!("CurrentTime: {}", current_time);
        let sched = Scheduler::new(UserNotifyHandler {
            remote: remote.clone(),
            api: apiclient.clone()
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
            remote, api: apiclient.clone(), dparse: core::JpDepsParser::new(), bot_userid: botinfo.id.clone(),
            call_checker: core::CallingSentenceDetector::new(&botinfo.id), rewriter: core::Rewriter::new(vec![
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
    fn on_message(&mut self, e: MessageEvent) {
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
            trace!("書き替え適用後の残りのペイロード:{}", rest_fmt);
            trace!("ペイロードをパース:");

            let mut rest: &str = &rest_fmt;
            if pv != Some(core::Preverb::FirstContact) {
                let (pv2, rest2) = self.call_checker.strip_preverbs_after(&rest);
                pv = pv2.or(pv); rest = rest2;
            }
            if pv == Some(core::Preverb::FirstContact) {
                if let Some(caps) = self.greeting_rx.captures(&rest) {
                    let gh_username = String::from(caps.get(1).unwrap().as_str());
                    let exec = core::CommandExecutor::new(&self.remote);
                    let reply_to_user = ReplyUserIdentity::from(&e);
                    let src_text = e.text.into_owned();

                    async_std::task::spawn(async move {
                        match exec.first_contact(&gh_username, &reply_to_user.user).await {
                            Ok(s) => { reply_to_async(&reply_to_user, &s).await; },
                            Err(err) => { report_error_async(&reply_to_user, &src_text, &format!("{:?}", err)).await; }
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
            let funcalls: Vec<_> = root_chunks.into_iter()
                .filter_map(|ci| match pv { /* Preverbによって変形する */
                    Some(core::Preverb::Tell) => core::CallingForm::Verb(
                        "教える",
                        core::VerbModifier::default(),
                        core::JpCallingEnv::from(
                            core::JpArguments::from(core::JpArgument::parse_chunk(&cabotree, ci, &linked_chunks))
                        )
                    ).into(),
                    _ => core::CallingForm::parse(&cabotree, ci, &linked_chunks)
                })
                .collect();
            let callform_parse_time = process_start.elapsed() - n0;
            debug!("ExecutionTime Summary:");
            debug!("* PrecallChecker: {} ms", as_millis_fract(&checker_time));
            debug!("* Rewriter:       {} ms", as_millis_fract(&rewriter_time));
            debug!("* JpDepParse:     {} ms", as_millis_fract(&dparse_time));
            debug!("* CallFormParse:  {} ms", as_millis_fract(&callform_parse_time));
            println!("callforms:");
            for f in &funcalls {
                f.pretty_print(&mut std::io::stdout());
                std::io::stdout().flush().unwrap();
            }
            if funcalls.is_empty() {
                let msg = ["なぁに？", "呼んだー？", "呼んだ？", "どうしたの？"]
                    .choose(&mut rand::thread_rng()).unwrap();
                async_std::task::spawn(reply_to_async(&From::from(&e), msg));
            } else {
                for f in funcalls.iter().map(core::Command::encode) {
                    let exec = core::CommandExecutor::new(&self.remote);
                    let apiclient = self.api.clone();
                    let reply_to_user = ReplyUserIdentity::from(&e);
                    let src_text = String::from(&e.text as &str);

                    async_std::task::spawn(async move {
                        let res = match f {
                            Ok(x) => match exec.execute(x, core::User::Slack(&reply_to_user.user)).await {
                                Ok(s) => { reply_to_async(&reply_to_user, &s).await; Ok(()) },
                                Err(e) => Err(e)
                            },
                            Err(e) => Err(core::ProcessError::RecognizationFailure(e))
                        };

                        if let Err(e) = res {
                            match e {
                                core::ProcessError::RecognizationFailure(msg) => {
                                    reply_to(&apiclient, &reply_to_user, &format!("ごめんね、{}", msg)).await;
                                },
                                core::ProcessError::UserUnidentifiable =>  {
                                    reply_to(&apiclient, &reply_to_user, "だれ......？").await;
                                },
                                core::ProcessError::UncountableObjective => {
                                    reply_to_async(&reply_to_user, "数えられないよ！？").await;
                                },
                                core::ProcessError::Network(ne) => {
                                    report_error(
                                        &apiclient, &reply_to_user, &src_text, &format!("NetworkError: {:?}", ne)
                                    ).await;
                                },
                                core::ProcessError::GithubAPIError(e) => {
                                    report_error(
                                        &apiclient, &reply_to_user, &src_text,
                                        &format!("Network Error while requesting to github api: {:?}", e)
                                    ).await;
                                }
                            }
                        }
                    });
                }
            }
        }
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

fn reply_to_async<'m>(src: &ReplyUserIdentity, msg: &str) -> impl std::future::Future<Output = bool> {
    use slack::SlackWebApi;

    let text = format!("<@{}> {}", src.user, msg);
    let p = slack::chat::PostMessage::new(&src.channel, &text).as_user(true).to_post_request();
    async move {
        surf::Client::new().send(p).await.expect("post err")
            .body_json::<slack::GenericResult>().await.expect("ilformed response").ok
    }
}
fn report_error_async(replying: &ReplyUserIdentity, text: &str, msg: &str) -> impl std::future::Future<Output = bool> {
    use slack::SlackWebApi;

    let text = format!("An error occured in replying to {:?}", text);
    let p = slack::chat::PostMessage::new(&replying.channel, &text)
        .as_user(true)
        .attachment(slack::chat::Attachment {
            color: Some("danger".into()), text: Some(msg.into()),
            .. Default::default()
        })
        .to_post_request();
    
    async move {
        surf::Client::new().send(p).await.expect("post err")
            .body_json::<slack::GenericResult>().await.expect("ilformed response").ok
    }
}

struct ReplyUserIdentity {
    channel: String,
    user: String
}
impl<'m> From<&'m MessageEvent<'m>> for ReplyUserIdentity {
    fn from(v: &MessageEvent) -> Self {
        ReplyUserIdentity {
            channel: String::from(v.channel),
            user: String::from(v.user)
        }
    }
}

async fn reply_to(api: &AsyncSlackApiSender, src: &ReplyUserIdentity, msg: &str) {
    api.send(chat::PostMessageParams {
        channel: &src.channel, text: &format!("<@{}> {}", &src.user, msg), as_user: Some(true), .. Default::default()
    }.to_apicall()).await;
}
async fn general_reply_to_id(api: &AsyncSlackApiSender, id: &str, msg: &str) {
    const RESPONSE_CHANNEL: &'static str = "CDQ0SLXG8";
    api.send(chat::PostMessageParams {
        channel: RESPONSE_CHANNEL, text: &format!("<@{}> {}", id, msg), as_user: Some(true), .. Default::default()
    }.to_apicall()).await;
}
async fn report_error(api: &AsyncSlackApiSender, replying: &ReplyUserIdentity, text: &str, msg: &str)
{
    api.send(chat::PostMessageParams {
        channel: &replying.channel, text: &format!("An error occured in replying to {:?}", text), as_user: Some(true),
        attachments: vec![chat::Attachment {
            color: Some("danger".into()), text: Some(msg.into()), .. Default::default()
        }], .. Default::default()
    }.to_apicall()).await;
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
