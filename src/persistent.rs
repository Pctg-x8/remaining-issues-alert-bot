//! Persistent Storage

use chrono::{NaiveTime, NaiveDateTime};
use diesel::*;
use diesel::sql_query;
use r2d2::Pool;
use r2d2_redis::RedisConnectionManager;
use redis::{ToRedisArgs, FromRedisValue, RedisResult, PipelineCommands};
use redis;

use diesel::{backend::Backend, deserialize::FromSql, deserialize::QueryableByName, row::NamedRow};
use diesel::sql_types as sqlty;
use diesel::deserialize::Result as DResult;

pub struct Persistent(Pool<diesel::r2d2::ConnectionManager<MysqlConnection>>, Pool<RedisConnectionManager>);

pub struct UserIotimes {
    pub github_id: i32, pub slack_id: String, pub github_login: String,
    pub intime: NaiveTime, pub outtime: NaiveTime
}
impl<DB: Backend<RawValue = [u8]>> QueryableByName<DB> for UserIotimes where NaiveTime: FromSql<sqlty::Time, DB> {
    fn build<R: NamedRow<DB>>(row: &R) -> DResult<Self> {
        Ok(UserIotimes {
            github_id: row.get::<sqlty::Integer, _>("github_id")?,
            slack_id: row.get::<sqlty::Varchar, _>("slack_id")?,
            github_login: row.get::<sqlty::Varchar, _>("github_login")?,
            intime: row.get::<sqlty::Time, _>("intime")?,
            outtime: row.get::<sqlty::Time, _>("outtime")?
        })
    }
}
pub struct LinkedGitHubInfo(i32, String);
impl<DB: Backend<RawValue = [u8]>> QueryableByName<DB> for LinkedGitHubInfo {
    fn build<R: diesel::row::NamedRow<DB>>(row: &R) -> DResult<Self> {
        Ok(LinkedGitHubInfo(row.get::<sqlty::Integer, _>("github_id")?, row.get::<sqlty::Varchar, _>("github_login")?))
    }
}
pub struct UserLastTimes { pub itime: NaiveDateTime, pub otime: NaiveDateTime }
impl<DB: Backend<RawValue = [u8]>> QueryableByName<DB> for UserLastTimes
        where NaiveDateTime: FromSql<sqlty::Datetime, DB> {
    fn build<R: NamedRow<DB>>(row: &R) -> DResult<Self> {
        Ok(UserLastTimes {
            itime: row.get::<sqlty::Datetime, _>("itime")?,
            otime: row.get::<sqlty::Datetime, _>("otime")?
        })
    }
}

pub struct RemainingWork { pub issue_id: u32, pub progress: Option<(u32, u32)> }
impl<'a> ToRedisArgs for &'a RemainingWork {
    fn write_redis_args(&self, out: &mut Vec<Vec<u8>>) {
        if let Some((c, t)) = self.progress {
            format!("{}:{}:{}", self.issue_id, c, t).write_redis_args(out);
        }
        else {
            self.issue_id.write_redis_args(out);
        }
    }
}
impl FromRedisValue for RemainingWork {
    fn from_redis_value(v: &redis::Value) -> RedisResult<Self> {
        if let redis::Value::Status(ref s) = v {
            let mut values = s.split(":");
            let issue_id = values.next().expect("Unable to get issue_id")
                .parse().expect("Unable to get issue_id as number");
            if let Some(cstr) = values.next() {
                let c = cstr.parse().expect("Unable to get checked_count as number");
                let t = values.next().expect("Missing total_checkbox_count")
                    .parse().expect("Unable to get total_checkbox_count as number");
                
                Ok(RemainingWork { issue_id, progress: Some((c, t)) })
            }
            else { Ok(RemainingWork { issue_id, progress: None }) }
        }
        else { panic!("Invalid data type") }
    }
}

macro_rules! PreparedSql {
    ($sql: expr; $($arg: expr => $sqlty: ty),+) => {
        sql_query($sql) $(.bind::<$sqlty, _>($arg))+
    }
}

impl Persistent {
    pub fn new() -> Self {
        let con = diesel::r2d2::ConnectionManager::new(env!("DATABASE_URL"));
        let rcon = RedisConnectionManager::new(env!("REDIS_URL")).unwrap();
        let pool = Pool::builder().build(con).expect("MySQL Connection failed");
        let rpool = Pool::builder().build(rcon).expect("Redis Connection failed");
        
        Persistent(pool, rpool)
    }

    pub fn init_user(&self, github_id: i32, slack_id: &str, github_login: &str) {
        let con = self.0.get().unwrap();

        PreparedSql!("Replace into usermap (github_id, slack_id, github_login) values (?, ?, ?)";
            github_id => sqlty::Integer, slack_id => sqlty::Varchar, github_login => sqlty::Varchar)
            .execute(&con).unwrap();
        PreparedSql!("Replace into user_inout_time (github_id, last_intime, last_outtime)
            values (?, utc_time(), timestamp(utc_date()-1, '10:00:00'))"; github_id => sqlty::Integer)
            .execute(&con).unwrap();
        PreparedSql!(
            "Replace into user_last_act_time (github_id, by_mention) values (?, null)"; github_id => sqlty::Integer)
            .execute(&con).unwrap();
    }
    pub fn forall_user_iotimes(&self) -> Vec<UserIotimes> {
        sql_query("Select usermap.github_id, slack_id, github_login, coalesce(temporal_intime, default_intime) as intime,
            coalesce(temporal_outtime, default_outtime) as outtime
            from user_inout_time right join usermap on usermap.github_id = user_inout_time.github_id")
            .load(&self.0.get().unwrap()).unwrap()
    }
    pub fn query_github_from_slack_id(&self, slack_id: &str) -> Option<(i32, String)> {
        let mut items =
            PreparedSql!("Select github_id, github_login from usermap where slack_id=? limit 1"; slack_id => sqlty::Varchar)
            .load::<LinkedGitHubInfo>(&self.0.get().unwrap()).unwrap();
        if items.is_empty() { None } else { let p = items.pop().unwrap(); (p.0, p.1).into() }
    }
    fn update_user_last_intime(&self, github_id: i32, last_intime: NaiveDateTime) {
        PreparedSql!("Update user_inout_time set last_intime=? where github_id=?";
            last_intime => sqlty::Datetime, github_id => sqlty::Integer).execute(&self.0.get().unwrap()).unwrap();
    }
    pub fn update_user_last_outtime(&self, github_id: i32, last_outtime: NaiveDateTime) {
        PreparedSql!("Update user_inout_time set last_outtime=? where github_id=?";
            last_outtime => sqlty::Datetime, github_id => sqlty::Integer).execute(&self.0.get().unwrap()).unwrap();
    }

    pub fn is_user_working(&self, github_id: i32) -> bool {
        let userstates = PreparedSql!("Select last_intime as itime, last_outtime as otime
            from user_inout_time where github_id=? limit 1"; github_id => sqlty::Integer)
            .load::<UserLastTimes>(&self.0.get().unwrap()).unwrap();
        if userstates.is_empty() { return false; }
        let ref userstate = userstates[0];
        userstate.itime > userstate.otime
    }

    pub fn user_setup_work(&self, github_id: i32, last_intime: NaiveDateTime, remaining_works: &[RemainingWork]) {
        self.update_user_last_intime(github_id, last_intime);
        let k = github_id.to_string();
        let con = self.1.get().unwrap();
        let _: () = redis::transaction(&*con, &[&k], |p| {
            for w in remaining_works { p.rpush(&k, w); }
            p.query(&*con)
        }).unwrap();
    }
    pub fn user_completed_works(&self, github_id: i32, remaining_works: &[RemainingWork]) {
        
    }
}
