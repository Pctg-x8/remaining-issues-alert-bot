//! Persistent Storage

use chrono::{NaiveTime, NaiveDateTime};
use diesel::*;
use diesel::sql_query;
use diesel::{Connection, OptionalExtension};
use ::r2d2::Pool;
use r2d2_redis::RedisConnectionManager;
use redis::{ToRedisArgs, FromRedisValue, RedisResult, RedisWrite, Commands};
use redis;

use diesel::deserialize::QueryableByName;
use diesel::sql_types as sqlty;

pub struct Persistent(Pool<diesel::r2d2::ConnectionManager<MysqlConnection>>, Pool<RedisConnectionManager>);

macro_rules! PreparedSql {
    ($sql: expr; $($arg: expr => $sqlty: ty),+) => {
        sql_query($sql) $(.bind::<$sqlty, _>($arg))+
    }
}

#[derive(QueryableByName)]
pub struct UserIotimes {
    #[sql_type = "sqlty::Integer"]
    pub github_id: i32,
    #[sql_type = "sqlty::Varchar"]
    pub slack_id: String,
    #[sql_type = "sqlty::Varchar"]
    pub github_login: String,
    #[sql_type = "sqlty::Time"]
    pub intime: NaiveTime,
    #[sql_type = "sqlty::Time"]
    pub outtime: NaiveTime
}
impl UserIotimes {
    pub fn fetch_all<C: Connection>(con: &C) -> Vec<Self> where Self: QueryableByName<C::Backend> {
        let select_cols = [
            "usermap.github_id", "slack_id", "github_login",
            "coalesce(temporal_intime, default_intime) as intime",
            "coalesce(temporal_outtime, default_outtime) as outtime"
        ];
        let datasource = "user_inout_time right join usermap on usermap.github_id = user_inout_time.github_id";

        sql_query(format!("Select {} from {}", select_cols.join(","), datasource)).load(con).unwrap()
    }
}
#[derive(QueryableByName)]
pub struct LinkedGitHubInfo {
    #[sql_type = "sqlty::Integer"]
    pub github_id: i32,
    #[sql_type = "sqlty::Varchar"]
    pub github_login: String
}
impl LinkedGitHubInfo {
    fn fetch1<C: Connection>(con: &C, slack_id: &str) -> Option<Self> where Self: QueryableByName<C::Backend> {
        PreparedSql!("Select github_id, github_login from usermap where slack_id=? limit 1"; slack_id => sqlty::Varchar)
            .get_result(con).optional().unwrap()
    }
}
#[derive(QueryableByName)]
pub struct UserLastTimes {
    #[sql_type = "sqlty::Datetime"]
    pub itime: NaiveDateTime,
    #[sql_type = "sqlty::Datetime"]
    pub otime: NaiveDateTime
}
impl UserLastTimes {
    pub fn fetch1<C: Connection>(con: &C, github_id: i32) -> Option<Self> where Self: QueryableByName<C::Backend> {
        PreparedSql!("Select last_intime as itime, last_outtime as otime
            from user_inout_time where github_id=? limit 1"; github_id => sqlty::Integer)
            .get_result::<Self>(con).optional().unwrap()
    }
}

pub struct RemainingWork { pub issue_id: u32, pub progress: Option<(u32, u32)> }
impl<'a> ToRedisArgs for &'a RemainingWork {
    fn write_redis_args<W: ?Sized + RedisWrite>(&self, out: &mut W) {
        if let Some((c, t)) = self.progress {
            format!("{}:{}:{}", self.issue_id, c, t).write_redis_args(out);
        } else {
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

impl Persistent {
    pub fn new(database_url: &str, redis_url: &str) -> Self {
        let con = diesel::r2d2::ConnectionManager::new(database_url);
        let rcon = RedisConnectionManager::new(redis_url).unwrap();
        let pool = Pool::builder().build(con).expect("PersistentDB Connection failed");
        let rpool = Pool::builder().build(rcon).expect("Redis Connection failed");
        
        Persistent(pool, rpool)
    }

    pub fn init_user(&self, github_id: i32, slack_id: &str, github_login: &str) {
        let con = self.0.get().unwrap();

        con.transaction(|| -> diesel::result::QueryResult<()> {
            sql_query("Replace into usermap (github_id, slack_id, github_login) values (?, ?, ?)")
                .bind::<sqlty::Integer, _>(github_id)
                .bind::<sqlty::Varchar, _>(slack_id)
                .bind::<sqlty::Varchar, _>(github_login)
                .execute(&con)?;
            PreparedSql!("Replace into user_inout_time (github_id, last_intime, last_outtime)
                values (?, utc_time(), timestamp(utc_date()-1, '10:00:00'))"; github_id => sqlty::Integer)
                .execute(&con)?;
            sql_query("Replace into user_last_act_time (github_id, by_mention) values (?, null)")
                .bind::<sqlty::Integer, _>(github_id)
                .execute(&con)?;
            
            Ok(())
        }).unwrap();
    }
    pub fn forall_user_iotimes(&self) -> Vec<UserIotimes> {
        UserIotimes::fetch_all(&self.0.get().unwrap())
    }
    /// Fetch a linked github user id and login name, by slack id.
    pub fn query_github_from_slack_id(&self, slack_id: &str) -> Option<(i32, String)> {
        LinkedGitHubInfo::fetch1(&self.0.get().unwrap(), slack_id).map(|l| (l.github_id, l.github_login))
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
        UserLastTimes::fetch1(&self.0.get().unwrap(), github_id).map_or(false, |st| st.itime > st.otime)
    }

    pub fn user_setup_work(&self, github_id: i32, last_intime: NaiveDateTime, remaining_works: &[RemainingWork]) {
        self.update_user_last_intime(github_id, last_intime);
        let k = github_id.to_string();
        let mut con = self.1.get().unwrap();
        let _: () = redis::transaction(&mut *con, &[&k], |con, p| {
            for w in remaining_works { p.rpush(&k, w); }
            p.query(con)
        }).unwrap();
    }
    pub fn user_moveout_works(&self, github_id: i32) -> Option<Vec<RemainingWork>> {
        self.1.get().unwrap().lrange(github_id.to_string(), 0, -1).unwrap()
    }
}
