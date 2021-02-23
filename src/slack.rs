//! Slack API

use serde::de::DeserializeOwned;
use std::{future::Future, pin::Pin};

#[derive(serde::Deserialize)]
pub struct GenericResult {
    pub ok: bool,
}

pub enum ApiResult<T> {
    Ok(T),
    Errored(String),
}
// https://github.com/serde-rs/serde/issues/880
impl<'d, T> serde::Deserialize<'d> for ApiResult<T>
where
    T: serde::Deserialize<'d>,
{
    fn deserialize<D: serde::de::Deserializer<'d>>(deserializer: D) -> Result<Self, D::Error> {
        let mut e = serde_json::Map::deserialize(deserializer)?;
        let ok = e
            .remove("ok")
            .ok_or_else(|| serde::de::Error::missing_field("ok"))
            .map(bool::deserialize)?
            .map_err(serde::de::Error::custom)?;
        if ok {
            T::deserialize(serde_json::Value::Object(e))
                .map(ApiResult::Ok)
                .map_err(serde::de::Error::custom)
        } else {
            e.remove("error")
                .ok_or_else(|| serde::de::Error::missing_field("error"))
                .map(String::deserialize)?
                .map_err(serde::de::Error::custom)
                .map(ApiResult::Errored)
        }
    }
}
impl<T> ApiResult<T> {
    pub fn into_result(self) -> Result<T, String> {
        match self {
            Self::Ok(v) => Ok(v),
            Self::Errored(e) => Err(e),
        }
    }
}

pub trait IApiSender {
    type Error;

    fn send<A: asyncslackbot::SlackWebAPI>(
        &self,
        call: &A,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ApiResult<()>, Self::Error>> + Send>> {
        self.send_apicall(call.to_apicall())
    }
    fn send_apicall(
        &self,
        call: asyncslackbot::SlackWebApiCall,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<()>, Self::Error>> + Send>>;
    fn send2<A: Api>(
        &self,
        call: &A,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<A::Response>, Self::Error>> + Send>>
    where
        A::Response: DeserializeOwned + 'static,
    {
        self.send_apicall2(call.to_apicall())
    }
    fn send_apicall2<R: DeserializeOwned + 'static>(
        &self,
        call: asyncslackbot::SlackWebApiCall,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<R>, Self::Error>> + Send>>;
}
impl<T: IApiSender> IApiSender for std::sync::Arc<T> {
    type Error = T::Error;

    fn send<A: asyncslackbot::SlackWebAPI>(
        &self,
        call: &A,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ApiResult<()>, Self::Error>> + Send>> {
        T::send(self, call)
    }
    fn send_apicall(
        &self,
        call: asyncslackbot::SlackWebApiCall,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<()>, Self::Error>> + Send>> {
        T::send_apicall(self, call)
    }
    fn send2<A: Api>(
        &self,
        call: &A,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<A::Response>, Self::Error>> + Send>>
    where
        A::Response: DeserializeOwned + 'static,
    {
        T::send2(self, call)
    }
    fn send_apicall2<R: DeserializeOwned + 'static>(
        &self,
        call: asyncslackbot::SlackWebApiCall,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<R>, Self::Error>> + Send>> {
        T::send_apicall2(self, call)
    }
}

pub struct ApiClient {
    auth_text: String,
    client: surf::Client,
}
impl ApiClient {
    pub fn new(tok: &str) -> Self {
        let client = surf::Client::new();
        // client.set_base_url(Url::parse("https://slack.com/api/").expect("invalid base url?"));

        Self {
            auth_text: format!("Bearer {}", tok),
            client,
        }
    }
}
impl IApiSender for ApiClient {
    type Error = surf::Error;

    fn send_apicall(
        &self,
        call: asyncslackbot::SlackWebApiCall,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<()>, surf::Error>> + Send>> {
        pub struct SlackWebApiCall {
            endpoint: &'static str,
            paramdata: String,
        }
        let call: SlackWebApiCall = unsafe { std::mem::transmute(call) };
        log::trace!("Post: {:?}", call.paramdata);
        let rb = self
            .client
            .post(call.endpoint)
            .header(surf::http::headers::AUTHORIZATION, self.auth_text.clone())
            .body(call.paramdata)
            .content_type(surf::http::mime::JSON);

        Box::pin(rb.recv_json())
    }
    fn send_apicall2<R: DeserializeOwned + 'static>(
        &self,
        call: asyncslackbot::SlackWebApiCall,
    ) -> Pin<Box<dyn Future<Output = Result<ApiResult<R>, surf::Error>> + Send>> {
        pub struct SlackWebApiCall {
            endpoint: &'static str,
            paramdata: String,
        }
        let call: SlackWebApiCall = unsafe { std::mem::transmute(call) };
        let rb = self
            .client
            .post(call.endpoint)
            .header(surf::http::headers::AUTHORIZATION, self.auth_text.clone())
            .body(call.paramdata)
            .content_type(surf::http::mime::JSON);

        Box::pin(rb.recv_json())
    }
}

#[derive(serde::Deserialize, Debug)]
pub struct ListedUser {
    pub id: String,
}
#[derive(serde::Deserialize, Debug)]
pub struct UsersList {
    pub resources: Vec<ListedUser>,
}

pub trait Api: asyncslackbot::SlackWebAPI {
    type Response;
}

#[derive(serde::Deserialize, Debug)]
pub struct AuthTestResponse {
    pub user: String,
    pub user_id: String,
}
#[derive(serde::Serialize)]
pub struct AuthTestRequest;
impl asyncslackbot::SlackWebAPI for AuthTestRequest {
    const EP: &'static str = "https://slack.com/api/auth.test";
}
impl Api for AuthTestRequest {
    type Response = AuthTestResponse;
}
