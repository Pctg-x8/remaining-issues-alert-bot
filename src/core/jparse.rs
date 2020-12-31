//! JpParser

use crate::cabocha as cabo;
use std::collections::{HashMap, BTreeMap};
use std::io::Write;
use regex::Regex;
use chrono::NaiveTime;

macro_rules! multitap {
    ($lhs: expr, eq_or, $($rhs: expr),*) => {
        { let lh = $lhs; $(lh == $rhs)||* }
    }
}

fn eq_iterator<A: Iterator, B: Iterator>(a: A, b: B) -> bool where A::Item: PartialEq<B::Item> {
    a.zip(b).all(|(a, b)| a == b)
}

fn token_as_numeric<N>(t: &cabo::Token) -> Option<Result<N, N::Err>> where N: std::str::FromStr {
    if eq_iterator(t.features().take(2), vec!["名詞", "数"].into_iter()) {
        Some(t.surface_normalized().parse())
    }
    else { None }
}

/// いつ
#[derive(Debug)]
pub struct TimeAt { time: NaiveTime, ambiguous: bool }
impl TimeAt {
    pub fn encode<'s>(a: &JpArgument<'s>) -> Result<Self, String> {
        let time = Self::encode_core(a)?;
        let ambiguous = a.text.iter().find(|t| multitap!(t.reading_form(), eq_or, Some("クライ"), Some("グライ"))).is_some()
            || a.mods.without_postposition.iter().find(|a| a.text.iter().find(|t| t.reading_form() == Some("オヨソ")).is_some()).is_some();
        Ok(TimeAt { time, ambiguous })
    }
    fn encode_core<'s>(a: &JpArgument<'s>) -> Result<NaiveTime, String> {
        let mut fragments = a.text.iter();
        if let Some(t0) = fragments.next().cloned().and_then(token_as_numeric) {
            let first_num = t0.map_err(|_| "何時かわからないや......".to_owned())?;
            match fragments.next() {
                Some(t) if t.reading_form() == Some("ジ") => {
                    let second_num = fragments.next().cloned().and_then(token_as_numeric).unwrap_or(Ok(0))
                        .map_err(|_| "何分かわからないや......".to_owned())?;
                    Ok(NaiveTime::from_hms(first_num, second_num, 0))
                },
                Some(t) if t.surface_normalized() == ":" => match fragments.next().cloned().and_then(token_as_numeric) {
                    Some(t2) => {
                        let second_num = t2.map_err(|_| "何分かわからないや......".to_owned())?;
                        Ok(NaiveTime::from_hms(first_num, second_num, 0))
                    },
                    _ => Err("時間指定間違えてたりしない？".to_owned())
                },
                _ => Err("何時かわからないや......".to_owned())
            }
        }
        else {
            Err("時間を指定してほしいな？".to_owned())
        }
    }
}

/// いつ(日付オフセット)
#[derive(Debug)] pub struct DayOffset(isize);
impl DayOffset {
    pub fn encode(reading: &str) -> Option<DayOffset> { 
        match reading {
            "アシタ" | "ツギ" => Some(DayOffset(1)),
            "キノウ" | "マエ" | "センジツ" => Some(DayOffset(-1)),
            "キョウ" | "イマ" => Some(DayOffset(0)),
            _ => None
        }
    }
}

/// 〜を教えて
#[derive(Debug)]
pub enum TellWhat {
    /// タスク、献立 引数は日付オフセット
    Tasks(String, bool, DayOffset),
    /// 〜の数を教えて(特殊形)
    CountOf(Box<TellWhat>),
    /// 数を教えて(適当に返す)
    RandomNumber,
    /// PR、プルリクエスト 両方falseの場合はopen状態のをランダムに返す
    PullRequests { opened_assignee: bool, unreviewed: bool, user_calls: String },
    /// 課題
    Issues { user_calls: String, ask_for_remains: bool, unresolved: bool }
}
impl TellWhat {
    fn encode_for1(noun: &str, mods: &JpArguments) -> Result<Self, String> {
        match noun {
            ntext @ "タスク" | ntext @ "メニュー" | ntext @ "献立" =>
                if let Some(a) = mods.any_first_postposition(&["の"]) {
                    if a.text.len() >= 1 {
                        let d = a.text.first().and_then(|&x| x.reading_form()).and_then(DayOffset::encode);
                        match d {
                            Some(d) => Ok(TellWhat::Tasks(format!("{}の{}", a.flat_text(), ntext), false, d)),
                            _ => Err(format!("「{}の{}」ってなに？", a.flat_text(), ntext))
                        }
                    }
                    else {
                        // とりあえず今日ので処理する
                        Ok(TellWhat::Tasks(format!("{}の{}", a.flat_text(), ntext), false, DayOffset(0)))
                    }
                }
                else if let Some(a) = mods.without_postposition.first() {
                    let remaining = a.first_standing_verb().map_or(false, |t| t.reading_form() == Some("ノコッ"));
                    Ok(TellWhat::Tasks(format!("{}{}", a.flat_text(), ntext), remaining, DayOffset(0)))
                }
                else { Ok(TellWhat::Tasks(ntext.to_owned(), false, DayOffset(0))) },
            "数" => match mods.any_first_postposition(&["の"]) {
                Some(p) => Self::encode(p).map(Self::count_of),
                None => Ok(TellWhat::RandomNumber)
            },
            "PR" => {
                fn descending(mods: &JpArguments, opened_assignee: &mut bool, unreviewed: &mut bool) -> Result<(), String> {
                    if mods.is_empty() { return Ok(()) }
                    else if let Some(a) = mods.any_first_postposition(&["の"]) {
                        if a.is_exact_reading_forms_of(&[Some("ミ"), Some("カクニン")]) {
                            *unreviewed = true;
                            descending(&a.mods, opened_assignee, unreviewed)
                        }
                        else {
                            Err(format!("「{}」ってどういう意味？", a.flat_text()))
                        }
                    }
                    else if let Some(a) = mods.without_postposition.first() {
                        match a.first_standing_verb() {
                            Some(t) if t.reading_form() == Some("ノコッ") => {
                                *opened_assignee = true;
                                descending(&a.mods, opened_assignee, unreviewed)
                            },
                            Some(v) => Err(format!("「{}」ってなに？", v.base_form())),
                            None => Err("言ってる意味がよくわからないや......(これ見えないはずだよね？)".to_owned())
                        }
                    }
                    else { Err("言ってる意味がよくわからないや......".to_owned()) }
                }

                let (mut opened_assignee, mut unreviewed) = (false, false);
                descending(mods, &mut opened_assignee, &mut unreviewed).map(|_| TellWhat::PullRequests {
                    opened_assignee, unreviewed,
                    user_calls: format!("{}PR", mods.to_string())
                })
            },
            "課題" => {
                fn descending(mods: &JpArguments, unresolved: &mut bool) -> Result<(), String> {
                    if mods.is_empty() { Ok(()) }
                    else if let Some(a) = mods.any_first_postposition(&["の"]) {
                        if a.is_exact_reading_forms_of(&[Some("ミ"), Some("カイケツ")]) {
                            *unresolved = true;
                            descending(&a.mods, unresolved)
                        }
                        else {
                            Err(format!("「{}」ってどういう意味？", a.flat_text()))
                        }
                    }
                    else {
                        Err("言ってる意味がよくわからないや......".to_owned())
                    }
                }

                let mut unresolved = false;
                descending(mods, &mut unresolved).map(|_| TellWhat::Issues {
                    unresolved,
                    user_calls: format!("{}課題", mods.to_string()),
                    ask_for_remains: mods.without_postposition.iter()
                        .find(|a| a.first_standing_verb().map_or(false, |t| t.reading_form() == Some("ノコッ"))).is_some()
                })
            }
            _ => Err(format!("「{}」についてはよくわからないんだー......", noun))
        }
    }
    fn encode(a: &JpArgument) -> Result<Self, String> {
        if a.text.len() != 1 {
            eprintln!("一つじゃないtextは未サポートですん");
            return Err(format!("「{}」についてはよくわからないんだー......", a.flat_text()));
        }

        Self::encode_for1(&a.text[0].surface_normalized(), &a.mods)
    }

    fn count_of(self) -> Self { TellWhat::CountOf(Box::new(self)) }
}
#[derive(Debug)]
pub enum Command
{
    /// おはよう/おはようございます など 丁寧語の場合trueになる(返答を若干変える)
    Greeting(bool),
    /// 「教える」(教える、教えて、教えて欲しい、など)コマンド
    /// なに？/ある？もかねる
    Tell(Option<TellWhat>),
    /// 「帰る」
    EndWorking(Option<TimeAt>, DayOffset)
}
impl Command
{
    pub fn encode<'s>(form: &CallingForm<'s>) -> Result<Self, String>
    {
        match form
        {
            CallingForm::Verb("教える", _, e) => {
                let arg1 = e.args.any_first_postposition(&["を", "について"])
                    .or(e.args.without_postposition.first())
                    .map(TellWhat::encode);
                Ok(Command::Tell(if let Some(a) = arg1 { Some(a?) } else { None }))
            },
            CallingForm::Verb("帰る", _, e) => {
                let d = e.args.any_first_postposition(&["は"]).and_then(|x| match &x.text[..] {
                    [t] => DayOffset::encode(t.reading_form().unwrap()), _ => None
                }).unwrap_or(DayOffset(0));
                let a1 = e.args.any_first_postposition(&["に", "で"]).map(TimeAt::encode);
                Ok(Command::EndWorking(if let Some(at) = a1 { Some(at?) } else { None }, d))
            },
            CallingForm::Kando(k, fdm) => Self::encode_kando(k, *fdm),
            CallingForm::MeishiQuery("ナニ", e) => {
                let a1 = e.args.any_first_postposition(&["って", "は"])
                    .or(e.args.without_postposition.first())
                    .map(TellWhat::encode);
                Ok(Command::Tell(if let Some(a) = a1 { Some(a?) } else { None }))
            },
            CallingForm::MeishiQuery("イクツ", e) => {
                let a1 = e.args.any_first_postposition(&["って", "は"])
                    .or(e.args.without_postposition.first())
                    .map(TellWhat::encode);
                Ok(Command::Tell(if let Some(a) = a1 { Some(a?.count_of()) } else { None }))
            },
            CallingForm::Verb("ある", _, e) => Self::encode_exists(e),
            CallingForm::Verb(n, _, _) => Err(format!("「{}」ってなに？", n)),
            CallingForm::MeishiQuery(n, _) => Err(format!("「{}」？", n)),
            CallingForm::ObjectNameQuery(n, e) => Ok(Command::Tell(Some(TellWhat::encode_for1(n, &e.args)?))),
            CallingForm::PeopleNameQuery("こゆき", _) => Err(format!("私がどうかしたの？")),
            CallingForm::PeopleNameQuery(n, _) => Err(format!("「{}」ってだれ？", n))
        }
    }
    fn encode_kando<'s>(kando: &'s str, following_desumasu: bool) -> Result<Self, String> {
        match kando {
            // おはよう(following_desumasu=false)/おはようございます(following_desumasu=true)
            "おはよう" => Ok(Command::Greeting(following_desumasu)),
            _ => Err(format!("なに話してくれたのかちょっとわからないや......"))
        }
    }
    // ある？
    fn encode_exists<'s>(env: &JpCallingEnv<'s>) -> Result<Self, String> {
        if env.args.is_empty() && env.prenoun.is_none() {
            // ある？だけ言われても困る
            return Err("言ってる意味がよくわからないんだけど......".to_owned())
        }
        let number_query = env.args.without_postposition.iter()
            .find(|x| x.is_reading_form1_of("イクツ") || x.is_reading_form1_of("イクツカ")).is_some();
        
        let tw = if let Some(x) = env.prenoun {
            Some(TellWhat::encode_for1(x.surface_normalized(), &env.args)?)
        }
        else if let Some(x) = env.args.any_first_postposition(&["って", "は", "が"]).map(TellWhat::encode) {
            Some(x?)
        }
        else { None };
        Ok(Command::Tell(if number_query { tw.map(TellWhat::count_of) } else { tw }))
    }
}

pub struct JpArgument<'s> {
    mods: JpArguments<'s>, text: Vec<&'s cabo::Token>
}
#[derive(Debug)]
pub struct JpArguments<'s> {
    with_postposition: HashMap<&'s str, Vec<JpArgument<'s>>>,
    without_postposition: Vec<JpArgument<'s>>
}
impl<'s> JpArgument<'s> {
    pub fn parse_chunk(tree: &'s cabo::TreeRef, chunk_index: usize, linkmap: &BTreeMap<usize, Vec<usize>>)
            -> (Self, Option<&'s str>) {
        let mods = linkmap.get(&chunk_index).map(|cs| JpArguments::parse_chunks(tree, cs, linkmap))
            .unwrap_or_else(JpArguments::new);
        let c = tree.chunk(chunk_index).unwrap();
        let mut tokens: Vec<_> = c.tokens(tree).collect();
        if tokens.last().map_or(false, |t| {
            let ts = t.features().take(2).collect::<Vec<_>>();
            ts[0] == "助詞" || ts[1] == "助詞類接続"
        }) {
            let pp = tokens.pop().unwrap();
            (JpArgument { mods, text: tokens }, Some(pp.surface_normalized()))
        }
        else { (JpArgument { mods, text: tokens }, None) }
    }

    pub fn is_reading_form1_of(&self, reading: &str) -> bool {
        self.text.len() == 1 && self.text[0].reading_form() == Some(reading)
    }
    pub fn is_exact_reading_forms_of(&self, readings: &[Option<&str>]) -> bool {
        self.text.len() == readings.len() && self.text.iter().zip(readings.iter()).all(|(a, &b)| a.reading_form() == b)
    }
    pub fn first_standing_verb(&self) -> Option<&cabo::Token> {
        self.text.iter().find(|t| eq_iterator(t.features().take(2), vec!["動詞", "自立"].into_iter())).cloned()
    }

    fn pretty_print<W: Write>(&self, writer: &mut W, depth: usize) {
        let tokens_chain = self.text.iter().map(|t| {
            format!("{}<{}>", t.surface_normalized(), t.features().take(2).collect::<Vec<_>>().join("/"))
        }).collect::<Vec<_>>().join("-");
        writeln!(writer, "{}", tokens_chain).unwrap();
        self.mods.pretty_print(writer, depth + 1);
    }
    fn flat_text(&self) -> String {
        self.text.iter().map(|&x| x.surface_normalized()).collect::<Vec<_>>().join("")
    }
}
impl<'s> ToString for JpArgument<'s> {
    fn to_string(&self) -> String {
        self.mods.to_string()
            + &self.text.iter().map(|x| x.surface_normalized()).collect::<Vec<_>>().join("")
    }
}
impl<'s> ToString for JpArguments<'s> {
    fn to_string(&self) -> String {
        self.with_postposition.iter().map(|(a, b)| b.iter().map(|x| x.to_string() + a).collect::<Vec<_>>().join(""))
            .chain(self.without_postposition.iter().map(ToString::to_string)).collect::<Vec<_>>().join("")
    }
}
impl<'s> JpArguments<'s> {
    pub fn new() -> Self {
        JpArguments { with_postposition: HashMap::new(), without_postposition: Vec::new() }
    }
    pub fn parse_chunks(tree: &'s cabo::TreeRef, chunks: &[usize], linkmap: &BTreeMap<usize, Vec<usize>>) -> Self {
        let mut this = Self::new();

        for &chunk_index in chunks {
            let (a, pp) = JpArgument::parse_chunk(tree, chunk_index, linkmap);
            if let Some(pp) = pp {
                this.with_postposition.entry(pp).or_insert_with(Vec::new).push(a);
            }
            else { this.without_postposition.push(a); }
        }
        return this;
    }
    pub fn is_empty(&self) -> bool { self.with_postposition.is_empty() && self.without_postposition.is_empty() }

    // querying //
    pub fn any_first_postposition(&self, pps: &[&str]) -> Option<&JpArgument<'s>> {
        pps.iter().filter_map(|&pp| self.with_postposition.get(pp)).map(|v| v.first().unwrap()).next()
    }

    fn pretty_print<W: Write>(&self, writer: &mut W, depth: usize) {
        for (a, bs) in &self.with_postposition {
            for b in bs {
                write!(writer, "{}->{}: ", std::iter::repeat(' ').take(depth * 2).collect::<String>(), a).unwrap();
                b.pretty_print(writer, depth);
            }
        }
        for a in &self.without_postposition {
            write!(writer, "{}~>", std::iter::repeat(' ').take(depth * 2).collect::<String>()).unwrap();
            a.pretty_print(writer, depth);
        }
    }
}
impl<'s> From<(JpArgument<'s>, Option<&'s str>)> for JpArguments<'s> {
    fn from((arg, pp): (JpArgument<'s>, Option<&'s str>)) -> Self {
        if let Some(pp) = pp {
            let mut v = HashMap::with_capacity(1); v.insert(pp, vec![arg]);
            JpArguments { with_postposition: v, without_postposition: Vec::new() }
        }
        else {
            JpArguments { without_postposition: vec![arg], with_postposition: HashMap::new() }
        }
    }
}

impl<'s> std::fmt::Debug for JpArgument<'s> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        let tokens_chain = self.text.iter().map(|t| {
            format!("{}<{}>", t.surface_normalized(), t.features().take(2).collect::<Vec<_>>().join("/"))
        }).collect::<Vec<_>>().join("-");

        if !self.mods.is_empty() { write!(fmt, "{:?}~>{}", self.mods, tokens_chain) }
        else { write!(fmt, "{}", tokens_chain) }
    }
}
pub struct JpCallingEnv<'s> { args: JpArguments<'s>, prenoun: Option<&'s cabo::Token> }
impl<'s> JpCallingEnv<'s> {
    fn parse(tree: &'s cabo::TreeRef, verb_chunk: usize, linkmap: &BTreeMap<usize, Vec<usize>>) -> Self {
        let verbchunk = tree.chunk(verb_chunk).unwrap();
        let prenoun = if tree.token(verbchunk.token_pos).unwrap().primary_part() == "名詞" &&
            tree.token(verbchunk.token_pos + 1).map_or(false, |t| t.primary_part() == "動詞") {
            Some(tree.token(verbchunk.token_pos).unwrap())
        }
        else { None };
        let args = linkmap.get(&verb_chunk).map(|acs| JpArguments::parse_chunks(tree, acs, linkmap))
            .unwrap_or_else(JpArguments::new);

        JpCallingEnv { args, prenoun }
    }
    // fn new() -> Self { JpCallingEnv { args: JpArguments::new(), prenoun: None } }

    fn pretty_print<W: Write>(&self, writer: &mut W, primary_form: &str) {
        if let Some(pn) = self.prenoun { writeln!(writer, "<{}>{}", pn.reading_form().unwrap(), primary_form).unwrap(); }
        else { writeln!(writer, "{}", primary_form).unwrap(); }
        self.args.pretty_print(writer, 0);
    }
}
impl<'s> From<JpArguments<'s>> for JpCallingEnv<'s> {
    fn from(args: JpArguments<'s>) -> Self { JpCallingEnv { args, prenoun: None } }
}
impl<'s> std::fmt::Debug for JpCallingEnv<'s> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(ref n) = self.prenoun {
            write!(fmt, "JpCallingEnv({:?}) prenoun={}", self.args, n.surface_normalized())
        }
        else { self.args.fmt(fmt) }
    }
}

#[derive(Debug)]
pub struct VerbModifier { negative: bool, done: bool }
impl Default for VerbModifier {
    fn default() -> Self { VerbModifier { negative: false, done: false } }
}

#[derive(Debug)]
pub enum CallingForm<'s> {
    Verb(&'s str, VerbModifier, JpCallingEnv<'s>), MeishiQuery(&'s str, JpCallingEnv<'s>), Kando(&'s str, bool),
    /// <名詞> 「は」 「?」
    ObjectNameQuery(&'s str, JpCallingEnv<'s>),
    /// ObjectNameQueryの特殊バージョン: <名詞/固有名詞/人名> (<名詞/接尾/人名>)* 「は」 「?」
    PeopleNameQuery(&'s str, JpCallingEnv<'s>)
}
impl<'s> CallingForm<'s> {
    pub fn parse(tree: &'s cabo::TreeRef, primary_chunk: usize, linkmap: &BTreeMap<usize, Vec<usize>>) -> Option<Self> {
        let pchunk = tree.chunk(primary_chunk).unwrap();
        let mut tokens = pchunk.tokens(tree);
        let is_anon_query_form = tokens.len() >= 2 &&
            tokens.at(tokens.len() - 2).map_or(false, |t| multitap!(t.surface_normalized(), eq_or, "は", "って")) &&
            tokens.at(tokens.len() - 1).map_or(false, |t| t.surface_normalized() == "?");
        if is_anon_query_form {
            if tokens.at(0).map_or(false, |t| eq_iterator(t.features().take(3), vec!["名詞", "固有名詞", "人名"].into_iter())) {
                let env = JpCallingEnv::parse(tree, primary_chunk, linkmap);
                return CallingForm::PeopleNameQuery(tokens.at(0).unwrap().surface_normalized(), env).into();
            }
            else if tokens.at(0).map_or(false, |t| t.primary_part() == "名詞") {
                let env = JpCallingEnv::parse(tree, primary_chunk, linkmap);
                return CallingForm::ObjectNameQuery(tokens.at(0).unwrap().surface_normalized(), env).into();
            }
        }
        if tokens.at(1).map_or(false, |t| t.surface_normalized() == "?") &&
            tokens.at(0).map_or(false, |t| t.primary_part() == "名詞") {
            return CallingForm::MeishiQuery(tokens.at(0).unwrap().reading_form().unwrap(),
                JpCallingEnv::parse(tree, primary_chunk, linkmap)).into();
        }
        while let Some(tok) = tokens.next() {
            if eq_iterator(tok.features().take(2), vec!["動詞", "自立"].into_iter()) {
                // verb
                let mut mods = VerbModifier::default();
                let mut tok_modifiers = tokens.filter(|x| x.primary_part() == "助動詞" ||
                    eq_iterator(x.features().take(2), vec!["動詞", "接尾"].into_iter()) ||
                    eq_iterator(x.features().take(2), vec!["動詞", "非自立"].into_iter()));
                while let Some(tok) = tok_modifiers.next() {
                    match tok.base_form() {
                        "た" => { mods.done = true; },
                        "ない" => { mods.negative = true; },
                        _ => ()
                    }
                }
                return CallingForm::Verb(tok.base_form(), mods, JpCallingEnv::parse(tree, primary_chunk, linkmap)).into();
            }
            else if tok.primary_part() == "感動詞" {
                // kando
                let following_desumasu = tokens
                    .find(|t| t.surface_normalized() == "ます" || t.surface_normalized() == "です").is_some();
                return CallingForm::Kando(tok.base_form(), following_desumasu).into();
            }
        }
        return None;
    }

    pub fn pretty_print<W: Write>(&self, writer: &mut W) {
        match self {
            CallingForm::Verb(s, m, e) => e.pretty_print(writer, &format!("Verb({}, {:?})", s, m)),
            CallingForm::MeishiQuery(s, e) => e.pretty_print(writer, &format!("MeishiQ({})", s)),
            CallingForm::ObjectNameQuery(s, e) => e.pretty_print(writer, &format!("ObjectNameQ({})", s)),
            CallingForm::PeopleNameQuery(s, e) => e.pretty_print(writer, &format!("PeopleNameQ({})", s)),
            CallingForm::Kando(s, dm) => writeln!(writer, "Kando({}, following_desumasu={})", s, dm).unwrap()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallBy { Name, Mention }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preverb { Tell, FirstContact }
/// 呼び出し文検知ロジック
pub struct CallingSentenceDetector {
    name_rx: regex::Regex, mention_header: String,
    tell_head_rx: regex::Regex, firstcontact_rx: Regex
}
impl CallingSentenceDetector {
    pub fn new(mention_id: &str) -> Self {
        let name_rx = Regex::new(r#"^(こ[\-ー~〜]*ゆ[\-ー~〜]*き[\-ー~〜]*(ち[\-ー~〜]*ゃ[\-ー~〜]*ん[\-ー~〜]*|さ[\-ー~〜]*ん[\-ー~〜]*)[!！、,  ?？]?)+"#).unwrap();
        let tell_head_rx = Regex::new(r#"^(教えて|おしえて)[ー\-〜~、,！!  ]*"#).unwrap();
        let firstcontact_rx = Regex::new(r#"^(はじめまして|初めまして)[！! 、,\-ー~〜 っ]*"#).unwrap();

        CallingSentenceDetector { name_rx, tell_head_rx, firstcontact_rx, mention_header: format!("<@{}>", mention_id) }
    }

    /// 「ねぇねぇ」「はい、」などの接頭語を切り落とす
    fn strip_precalls(input: &str) -> &str {
        fn strip_prefix<'s>(s: &'s str, p: &str) -> Option<&'s str> {
            if s.starts_with(p) { Some(&s[p.as_bytes().len()..]) } else { None }
        }

        if let Some(mut p0) = strip_prefix(input, "ねぇ").or_else(|| strip_prefix(input, "ねえ")) {
            while let Some(p1) = strip_prefix(p0, "ねぇ").or_else(|| strip_prefix(p0, "ねえ")) { p0 = p1; }
            let punctures = p0.chars().take_while(|&c| c == '、' || c == ',' || c == ' ' || c == ' '
                || c == '~' || c == '〜' || c == '-' || c == 'ー').fold(0, |c, b| c + b.len_utf8());
            return &p0[punctures..];
        }
        else if let Some(mut p0) = strip_prefix(input, "は") {
            let dashes = p0.chars().take_while(|&c| c == '~' || c == '〜' || c == '-' || c == 'ー')
                .fold(0, |c, b| c + b.len_utf8());
            p0 = &p0[dashes..];
            return strip_prefix(p0, "い、").or_else(|| strip_prefix(p0, "い,"))
                .or_else(|| strip_prefix(p0, "い ")).or_else(|| strip_prefix(p0, "い "))
                .unwrap_or(input);
        }
        else { input }
    }
    /// 教えて！など特殊な接頭(ねえねえ、などの呼びかけのあとしか反応しない)
    fn strip_precall_verbs<'s>(&self, input: &'s str) -> (Option<Preverb>, &'s str) {
        if let Some(nmatch) = self.tell_head_rx.find(input) { (Some(Preverb::Tell), &input[nmatch.end()..]) }
        else if let Some(nmatch) = self.firstcontact_rx.find(input) { (Some(Preverb::FirstContact), &input[nmatch.end()..]) }
        else { (None, input) }
    }
    pub fn check<'s>(&self, mut input: &'s str) -> Option<(Option<Preverb>, CallBy, &'s str)> {
        input = Self::strip_precalls(input);
        let (preverb, input) = self.strip_precall_verbs(input);
        if input.starts_with(&self.mention_header) {
            let input = &input[self.mention_header.as_bytes().len()..];
            let spaces = input.chars().take_while(|&c| c == ' ' || c == ' ' || c == '\n' || c == '\t')
                .fold(0, |a, c| a + c.len_utf8());
            return (preverb, CallBy::Mention, &input[spaces..]).into();
        }
        else {
            println!("名前判定に食わせる: {:?}", input);
            if let Some(nmatch) = self.name_rx.find(input) {
                return (preverb, CallBy::Name, &input[nmatch.end()..]).into();
            }
        }

        return None;
    }
    pub fn strip_preverbs_after<'s>(&self, input: &'s str) -> (Option<Preverb>, &'s str) {
        if let Some(nmatch) = self.firstcontact_rx.find(&input) {
            (Preverb::FirstContact.into(), &input[nmatch.end()..])
        }
        else { (None, input) }
    }
}

use std::borrow::Cow;
/// Rewriteルール
pub struct Rewriter<'s> { rules: Vec<(Regex, fn(&regex::Captures) -> Cow<'s, str>)> }
impl<'s> Rewriter<'s> {
    pub fn new(rules: Vec<(&str, fn(&regex::Captures) -> Cow<'s, str>)>) -> Self {
        Rewriter { rules: rules.into_iter().map(|(r, d)| (Regex::new(r).unwrap(), d)).collect() }
    }
    pub fn rewrite<'p>(&self, src: &'p str) -> Cow<'p, str> {
        let mut returns = Cow::Borrowed(src);
        for (p, d) in &self.rules {
            returns = match returns {
                Cow::Borrowed(r) => p.replace_all(r, *d),
                Cow::Owned(r) => Cow::Owned(match p.replace_all(&r, *d) {
                    Cow::Borrowed(_) => r,
                    Cow::Owned(p) => p
                })
            };
        }
        return returns;
    }
}

use cabo::Cabocha;
/// 係り受け解析(CaboCha Wrapper)
pub struct JpDepsParser(Cabocha);
impl JpDepsParser {
    pub fn new() -> Self { JpDepsParser(Cabocha::new("-u commands.dic")) }
    pub fn parse(&self, input: &str) -> cabo::TreeRef { self.0.parse_str_to_tree(input) }
}
impl cabo::TreeRef {
    pub fn build_reverse_deps_tree(&self) -> (Vec<usize>, BTreeMap<usize, Vec<usize>>) {
        let (mut root_chunks, mut linked_chunks) = (Vec::new(), BTreeMap::new());
        for (ci, c) in self.chunks().enumerate() {
            if c.is_root() { root_chunks.push(ci); } // root
            else { linked_chunks.entry(c.link as usize).or_insert_with(Vec::new).push(ci); }
        }
        return (root_chunks, linked_chunks);
    }
}
