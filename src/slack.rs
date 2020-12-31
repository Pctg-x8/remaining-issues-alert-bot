//! Slack API

use serde::Serialize;

fn bearer_header_value() -> String { format!("Bearer {}", std::env::var("SLACK_API_TOKEN").expect("SLACK_API_TOKEN not set")) }

pub trait SlackWebApi: Serialize + Sized {
    const EP: &'static str;

    fn to_post_request(&self) -> surf::Request {
        let mut r = surf::Request::new(surf::http::Method::Post, surf::Url::parse(Self::EP).expect("invalid ep url"));
        r.body_json(self).expect("Failed to serialize Slack request");
        r.append_header(surf::http::headers::CONTENT_TYPE, "application/json");
        r.append_header(surf::http::headers::AUTHORIZATION, &bearer_header_value());

        r
    }
}
#[derive(serde::Deserialize)]
pub struct GenericResult { pub ok: bool }
pub fn send<P: SlackWebApi>(params: P) -> impl std::future::Future<Output = surf::Result<bool>> {
    let rq = params.to_post_request();
    async move { surf::Client::new().send(rq).await?.body_json::<GenericResult>().await.map(|r| r.ok) }
}

pub mod chat
{
    use std::borrow::Cow;

    #[derive(serde::Serialize)]
    pub struct PostMessage<'s>
    {
        pub channel: &'s str, pub text: &'s str,
        pub as_user: Option<bool>,
        pub attachments: Vec<Attachment<'s>>
    }
    impl<'s> super::SlackWebApi for PostMessage<'s>
    {
        const EP: &'static str = "https://slack.com/api/chat.postMessage";
    }
    impl<'s> PostMessage<'s>
    {
        pub fn new(channel: &'s str, text: &'s str) -> Self
        {
            PostMessage
            {
                channel, text,
                as_user: None, attachments: Vec::new()
            }
        }
        pub fn as_user(mut self, enable: bool) -> Self
        {
            self.as_user = Some(enable);
            self
        }
        pub fn attachment(mut self, a: Attachment<'s>) -> Self
        {
            self.attachments.push(a);
            self
        }
    }
    
    #[derive(serde::Serialize, serde::Deserialize, Debug)]
    pub struct Attachment<'s>
    {
        pub color: Option<Cow<'s, str>>, pub text: Option<Cow<'s, str>>
    }
    impl<'s> Default for Attachment<'s>
    {
        fn default() -> Self
        {
            Attachment { color: None, text: None }
        }
    }
}
