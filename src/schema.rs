table! {
    user_inout_time (github_id) {
        github_id -> Integer,
        last_intime -> Datetime,
        last_outtime -> Datetime,
    }
}

table! {
    user_last_act_time (github_id) {
        github_id -> Integer,
        by_mention -> Nullable<Bool>,
    }
}

table! {
    usermap (github_id) {
        github_id -> Integer,
        slack_id -> Varchar,
        github_login -> Varchar,
    }
}

allow_tables_to_appear_in_same_query!(
    user_inout_time,
    user_last_act_time,
    usermap,
);
