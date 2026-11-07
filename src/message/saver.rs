use std::path::PathBuf;
use std::{collections::HashSet, fs};

use chrono::{Local, NaiveDateTime};
use rayon::prelude::*;
use regex::Regex;
use walkdir::{DirEntry, WalkDir};

use crate::http::timeline::Timeline;
use crate::{
    errors::*,
    http::{self, client::SHNClient, groups::Groups, tags::Tags, timeline::TimelineMessages},
    message,
    message::file::{Picture, SaveToFile, Text, Video, Voice},
    Config, Kind,
};

lazy_static! {
    static ref ID_DATE_REGEX: Regex =
        Regex::new(r"(?x)(?P<id>\d+)_(?P<media>\d)_(?P<date>\d+)").unwrap();
}

pub struct Saver<'a, C: SHNClient> {
    config: &'a Config<C>,
}

impl<'b, C: SHNClient> Saver<'b, C> {
    pub fn new<'a>(config: &'a Config<C>) -> Saver<'a, C> {
        Saver { config }
    }

    pub fn save(&self) -> Result<()> {
        if let Some(member_id) = self.config.member_id {
            self.save_direct_member(member_id)?;
            return Ok(());
        }

        if let Some(message_group_id) = self.config.message_group_id {
            self.save_direct_group(message_group_id)?;
            return Ok(());
        }

        let groups = http::groups::request(self.config.client.clone(), &self.config.access_token)?;
        let tags = http::tags::request(self.config.client.clone(), &self.config.access_token)?;

        // TODO: 並列処理したい
        // 購読しているメンバー毎にメッセージを保存するためのループ
        for member_identifier in self.subscribed_list(&groups, &tags) {
            self.save_messages(member_identifier)?;
        }

        Ok(())
    }

    fn save_direct_member(&self, member_id: u32) -> Result<()> {
        let member = http::members::request(
            self.config.client.clone(),
            &self.config.access_token,
            &member_id,
        )?;
        let message_group_id = match self.config.message_group_id {
            Some(id) => id,
            None => *member
                .groups
                .first()
                .ok_or_else(|| format!("member {} has no message groups", member_id))?,
        };

        self.save_messages(MemberIdentifier::new(
            message_group_id,
            self.trim(&member.name),
            "".to_string(),
            true,
        ))
    }

    fn save_direct_group(&self, message_group_id: u32) -> Result<()> {
        let name = self
            .config
            .name
            .first()
            .map(|name| self.trim(name))
            .unwrap_or_else(|| format!("group_{}", message_group_id));

        self.save_messages(MemberIdentifier::new(
            message_group_id,
            name,
            "".to_string(),
            true,
        ))
    }

    fn subscribed_list(&self, group: &Vec<Groups>, tags: &Vec<Tags>) -> Vec<MemberIdentifier> {
        self.create_member_identifier_list(group, tags)
            .iter()
            .cloned()
            .filter(|m| m.subscription)
            .filter(|m| {
                if self.config.name.is_empty() {
                    return true;
                } // メンバー指定が無い場合は全メンバーを対象にする
                self.config.name.contains(&self.trim(&m.name))
            })
            .collect::<Vec<_>>()
    }

    fn create_member_identifier_list(
        &self,
        group: &Vec<Groups>,
        tags: &Vec<Tags>,
    ) -> Vec<MemberIdentifier> {
        let mut member_identifier_vec = Vec::with_capacity(group.len());
        group.iter().for_each(|g| {
            // もっといい書き方があるはず
            let mut group = "".to_string();
            let mut gen = "".to_string();
            tags.iter().for_each(|t| {
                let dimension = t.meta.as_ref().and_then(|meta| meta.dimension.as_ref());
                if g.tags.contains(&t.uuid) && dimension.is_some() {
                    group = t.name.clone();
                }
                if g.tags.contains(&t.uuid) && dimension.is_none() {
                    gen = t.name.clone();
                }
            });
            // 乃木坂の場合はg.tagsに世代情報(1期, 2期)が存在しないため全員乃木坂ディレクトリ以下に保存される
            member_identifier_vec.push(MemberIdentifier::new(
                g.id,
                self.trim(&g.name),
                gen,
                g.subscription.is_some(),
            ));
        });

        member_identifier_vec
    }

    fn trim(&self, str: &String) -> String {
        str.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    }

    fn save_messages(&self, member_identifier: MemberIdentifier) -> Result<()> {
        println!("saving messages of {}...", member_identifier.name);

        let member_dir_buf = self.create_member_dir_buf(&member_identifier)?;
        let mut id_dates = self.id_dates(&member_dir_buf);
        let mut fromdate = match self.config.from {
            Some(f) => f.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            None => self.latest_date(&id_dates)?,
        };

        // 購読開始から24時間前までに配信されたメッセージを保存する
        let past_messages = http::past_messages::request(
            self.config.client.clone(),
            &self.config.access_token,
            &member_identifier.id,
        )?;
        for message in &past_messages.messages {
            self.save_message(&message, &id_dates, &member_dir_buf)?
        }
        id_dates = self.id_dates(&member_dir_buf);

        let mut count = http::timeline::DEFAULT_COUNT;
        let mut continuation: Option<String> = None;
        let mut seen_continuations = HashSet::new();
        let mut uses_continuation = false;

        // 購読しているメンバーのメッセージを取得するAPIを複数回叩くためのループ
        loop {
            if self.is_from_after_to(&fromdate)? {
                break;
            }

            let timeline = http::timeline::request_page(
                self.config.client.clone(),
                &self.config.access_token,
                &member_identifier.id,
                &fromdate,
                &count.to_string(),
                continuation.as_deref(),
            )?;

            let message_length = timeline.messages.len();
            println!(
                "Fetched {} messages for {} (from {}, page size {}).",
                message_length, member_identifier.name, fromdate, count
            );
            let next_continuation = timeline
                .continuation
                .as_deref()
                .filter(|value| !value.is_empty());
            if message_length == 0 && next_continuation.is_none() {
                break;
            }

            let reached_to = self.timeline_reached_to(&timeline)?;

            // updated_atの値を基準にメッセージを取得している
            // 取得したメッセージのupdated_atがすべて同じだと基準が判明しない
            // 最新のメッセージまで取得出来たか、異なるupdated_atの値が現れるまでメッセージ取得数を増やしてメッセージ取得を施行する
            if !uses_continuation
                && next_continuation.is_none()
                && !reached_to
                && message_length >= count
                && self.are_all_updated_at_same(&timeline)
            {
                count += http::timeline::DEFAULT_COUNT;
                continue;
            }

            // メッセージを取得するAPIを叩くと複数件のメッセージを取得出来る
            // そのメッセージを1件ずつ処理するためのループ
            for message in &timeline.messages {
                self.save_message(&message, &id_dates, &member_dir_buf)?
            }
            id_dates = self.id_dates(&member_dir_buf);

            if reached_to {
                break;
            };
            // count includes letters/comments as well as messages. Even a page
            // with no downloadable messages can point to another page.
            if let Some(next) = next_continuation {
                if !seen_continuations.insert(next.to_string()) {
                    return Err(
                        "Timeline repeated a continuation token; crawl is incomplete".into(),
                    );
                }
                continuation = Some(next.to_string());
                uses_continuation = true;
                continue;
            }
            if uses_continuation {
                break;
            }
            let next_fromdate = self.latest_timeline_date(&timeline)?;
            let next_date = Self::parse_message_updated_at(&next_fromdate)?;
            let current_date = Self::parse_message_updated_at(&fromdate)?;
            if next_date < current_date {
                return Err("Timeline pagination moved backwards; crawl is incomplete".into());
            }
            // updated_from is inclusive. A short page containing only the cursor
            // timestamp confirms that there are no later messages. A short page
            // with a newer timestamp still needs another request (servers may cap
            // the page size below the requested count).
            if next_date == current_date {
                if message_length < count {
                    break;
                }
                return Err("Timeline pagination did not advance; crawl is incomplete".into());
            }
            fromdate = next_fromdate;

            // 保存し終わったらメッセージ取得数をデフォルトに戻す
            count = http::timeline::DEFAULT_COUNT;
        }
        println!("complete saving messages of {}!", &member_identifier.name);

        Ok(())
    }

    fn parse_message_updated_at(updated_at: &str) -> Result<NaiveDateTime> {
        Ok(NaiveDateTime::parse_from_str(
            updated_at,
            "%Y-%m-%dT%H:%M:%SZ",
        )?)
    }

    fn is_from_after_to(&self, fromdate: &str) -> Result<bool> {
        match self.config.to.as_ref() {
            Some(to) => Ok(&Self::parse_message_updated_at(fromdate)? > to),
            None => Ok(false),
        }
    }

    fn is_message_in_date_range(&self, message: &TimelineMessages) -> Result<bool> {
        let updated_at = Self::parse_message_updated_at(&message.updated_at)?;

        if let Some(from) = self.config.from.as_ref() {
            if &updated_at < from {
                return Ok(false);
            }
        }

        if let Some(to) = self.config.to.as_ref() {
            if &updated_at > to {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn timeline_reached_to(&self, timeline: &Timeline) -> Result<bool> {
        match self.config.to.as_ref() {
            Some(to) => timeline
                .messages
                .iter()
                .try_fold(false, |reached, message| {
                    if reached {
                        Ok(true)
                    } else {
                        Ok(&Self::parse_message_updated_at(&message.updated_at)? > to)
                    }
                }),
            None => Ok(false),
        }
    }

    fn latest_timeline_date(&self, timeline: &Timeline) -> Result<String> {
        let message = timeline
            .messages
            .iter()
            .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
            .ok_or_else(|| "timeline has no messages".to_string())?;
        Self::parse_message_updated_at(&message.updated_at)?;
        Ok(message.updated_at.clone())
    }

    fn create_member_dir_buf(&self, member_identifier: &MemberIdentifier) -> Result<PathBuf> {
        let mut parent_dir_buf = self.config.dir.clone();
        parent_dir_buf.push(&member_identifier.gen);

        let mut member_dir_buf = parent_dir_buf.clone();
        member_dir_buf.push(&member_identifier.name);

        if member_dir_buf.exists() {
            member_dir_buf = self.unique_member_dir_buf(&parent_dir_buf, &member_identifier.name);
        }

        println!("create directory: {}", member_dir_buf.display());
        fs::create_dir_all(&member_dir_buf)?;
        Ok(member_dir_buf)
    }

    fn unique_member_dir_buf(&self, parent_dir_buf: &PathBuf, member_name: &String) -> PathBuf {
        let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
        let base_name = format!("{}_{}", member_name, timestamp);
        let mut candidate = parent_dir_buf.clone();
        candidate.push(&base_name);

        let mut index = 2;
        while candidate.exists() {
            candidate = parent_dir_buf.clone();
            candidate.push(format!("{}_{}", base_name, index));
            index += 1;
        }

        candidate
    }

    fn save_message(
        &self,
        message: &TimelineMessages,
        id_dates: &Vec<IdDate>,
        member_dir_buf: &PathBuf,
    ) -> Result<()> {
        if !self.is_message_in_date_range(message)? {
            return Ok(());
        }

        let media = match message.messages_type.as_str() {
            "text" => 0,
            "picture" => 1,
            "video" => 2,
            "voice" => 3,
            "link" => 4,
            _ => {
                let err = format!("unknown type: {}", message.messages_type.as_str());
                return Err(err.into());
            }
        };

        // 既に保存済のファイルはAPIリクエストしない&上書き保存せずスルー
        if id_dates
            .iter()
            .any(|id_date| id_date.id == message.id && id_date.media == media)
        {
            return Ok(());
        }
        match message.messages_type.as_str() {
            "text" => {
                if !self.config.kind.contains(&Kind::Text) {
                    return Ok(());
                }
                let message_file_text = Text::new(
                    member_dir_buf,
                    message::file::file_name(&message.id, &0, &message.updated_at)?,
                    &message.text,
                );
                message_file_text.save()?
            }
            "picture" => {
                if !self.config.kind.contains(&Kind::Picture) {
                    return Ok(());
                }
                let message_file_picture = Picture::new(
                    member_dir_buf,
                    message::file::file_name(&message.id, &1, &message.updated_at)?,
                    &message.text,
                    &message.file,
                );
                message_file_picture.save()?
            }
            "video" => {
                if !self.config.kind.contains(&Kind::Video) {
                    return Ok(());
                }
                let message_file_video = Video::new(
                    member_dir_buf,
                    message::file::file_name(&message.id, &2, &message.updated_at)?,
                    &message.file,
                );
                message_file_video.save()?
            }
            "voice" => {
                if !self.config.kind.contains(&Kind::Voice) {
                    return Ok(());
                }
                let message_file_voice = Voice::new(
                    member_dir_buf,
                    message::file::file_name(&message.id, &3, &message.updated_at)?,
                    &message.file,
                );
                message_file_voice.save()?
            }
            "link" => {
                // リンク型はテキストファイルとして保存するが、種別は Link として扱う
                if !self.config.kind.contains(&Kind::Link) {
                    return Ok(());
                }
                let message_file_text = Text::new(
                    member_dir_buf,
                    message::file::file_name(&message.id, &4, &message.updated_at)?,
                    &message.text,
                );
                message_file_text.save()?
            }
            _ => {
                let err = format!("unknown type: {}", message.messages_type.as_str());
                return Err(err.into());
            }
        };

        Ok(())
    }

    fn id_dates(&self, dir_buf: &PathBuf) -> Vec<IdDate> {
        let mut result = WalkDir::new(dir_buf)
            .into_iter()
            .par_bridge()
            .filter(|r| !r.as_ref().unwrap().path().is_dir())
            .map(|r| {
                let dir_entry = r.unwrap();
                dir_entry_to_id_date(&dir_entry)
            })
            .flatten()
            .collect::<Vec<_>>();
        result.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then(a.id.cmp(&b.id))
                .then(a.media.cmp(&b.media))
        });
        result
    }

    fn latest_date(&self, id_dates: &Vec<IdDate>) -> Result<String> {
        if id_dates.is_empty() {
            return Ok(String::from("2000-01-01T09:00:00Z"));
        }
        let date = id_dates.last().unwrap().clone().date;
        let date = NaiveDateTime::parse_from_str(&date, "%Y%m%d%H%M%S");
        Ok(date?.format("%Y-%m-%dT%H:%M:%SZ").to_string())
    }

    fn are_all_updated_at_same(&self, timeline: &Timeline) -> bool {
        let first_updated_at = &timeline.messages[0].updated_at;
        timeline
            .messages
            .iter()
            .all(|message| &message.updated_at == first_updated_at)
    }
}

#[derive(Clone, Debug)]
pub struct MemberIdentifier {
    id: u32,
    name: String,
    gen: String,
    subscription: bool,
}

impl MemberIdentifier {
    pub fn new(id: u32, name: String, gen: String, subscription: bool) -> MemberIdentifier {
        MemberIdentifier {
            id,
            name,
            gen,
            subscription,
        }
    }
}

#[derive(Clone, Debug)]
struct IdDate {
    id: u32,
    media: u32,
    date: String,
}

fn dir_entry_to_id_date(filename: &DirEntry) -> Option<IdDate> {
    let re = ID_DATE_REGEX.clone();
    re.captures(filename.file_name().to_str().unwrap())
        .and_then(|cap| {
            Some(IdDate {
                id: cap["id"].parse::<u32>().unwrap(),
                media: cap["media"].parse::<u32>().unwrap(),
                date: cap["date"].to_string(),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{de::DeserializeOwned, Serialize};
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone)]
    struct MockClient {
        messages: Rc<Vec<serde_json::Value>>,
        calls: Rc<RefCell<Vec<String>>>,
        page_limit: usize,
    }

    impl SHNClient for MockClient {
        fn new() -> Self {
            unreachable!()
        }

        fn post_request<RT: DeserializeOwned, JT: Serialize + ?Sized>(
            &self,
            _: &str,
            _: &JT,
            _: bool,
        ) -> Result<RT> {
            unreachable!()
        }

        fn get_request<RT: DeserializeOwned>(
            &self,
            path: &str,
            _: &str,
            parameters: Option<Vec<(&str, &str)>>,
            _: bool,
        ) -> Result<RT> {
            let body = if path.ends_with("/past_messages") {
                serde_json::json!({"messages": []})
            } else {
                assert!(path.ends_with("/timeline"));
                let parameters = parameters.unwrap();
                let from = parameters
                    .iter()
                    .find(|(k, _)| *k == "updated_from")
                    .unwrap()
                    .1;
                let count = parameters
                    .iter()
                    .find(|(k, _)| *k == "count")
                    .unwrap()
                    .1
                    .parse::<usize>()
                    .unwrap();
                let mut calls = self.calls.borrow_mut();
                calls.push(from.to_string());
                assert!(calls.len() < 30, "pagination did not terminate");
                let messages: Vec<_> = self
                    .messages
                    .iter()
                    .filter(|m| m["updated_at"].as_str().unwrap() >= from)
                    .take(count.min(self.page_limit))
                    .collect();
                serde_json::json!({"messages": messages, "comments": [], "letters": [],
                    "queried_at": "2026-11-06T00:00:00Z"})
            };
            Ok(serde_json::from_value(body).unwrap())
        }
    }

    fn crawl(
        message_count: usize,
        page_limit: usize,
        shared_timestamp: bool,
        to: Option<NaiveDateTime>,
    ) -> (usize, Vec<String>) {
        let start =
            NaiveDateTime::parse_from_str("2026-01-01T00:00:00Z", "%Y-%m-%dT%H:%M:%SZ").unwrap();
        let messages = (0..message_count)
            .map(|i| {
                let seconds = if shared_timestamp { 0 } else { i as i64 };
                let date = (start + chrono::Duration::seconds(seconds))
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string();
                serde_json::json!({"id": i + 1, "group_id": 1, "is_favorite": false,
                "is_silent": false, "published_at": date, "updated_at": date,
                "state": "published", "type": "text", "text": "test"})
            })
            .collect();
        let calls = Rc::new(RefCell::new(Vec::new()));
        let directory = tempdir::TempDir::new("colmsg-pagination").unwrap();
        let config = Config {
            name: vec!["test".into()],
            from: None,
            to,
            kind: vec![Kind::Text],
            dir: directory.path().to_path_buf(),
            access_token: "test".into(),
            member_id: None,
            message_group_id: Some(1),
            client: MockClient {
                messages: Rc::new(messages),
                calls: calls.clone(),
                page_limit,
            },
        };
        Saver::new(&config).save().unwrap();
        let saved = fs::read_dir(directory.path().join("test")).unwrap().count();
        let requests = calls.borrow().clone();
        (saved, requests)
    }

    #[test]
    fn crawls_more_than_one_hundred_messages() {
        let (saved, calls) = crawl(251, 100, false, None);
        assert_eq!(saved, 251);
        assert!(calls.len() >= 4);
    }

    #[test]
    fn continues_after_server_returns_a_short_page() {
        let (saved, calls) = crawl(251, 37, false, None);
        assert_eq!(saved, 251);
        assert!(calls.len() > 7);
    }

    #[test]
    fn expands_pages_for_messages_sharing_a_timestamp() {
        let (saved, _) = crawl(250, usize::MAX, true, None);
        assert_eq!(saved, 250);
    }

    #[test]
    fn stops_at_explicit_end_date() {
        let to =
            NaiveDateTime::parse_from_str("2026-01-01T00:02:00Z", "%Y-%m-%dT%H:%M:%SZ").unwrap();
        let (saved, _) = crawl(251, 37, false, Some(to));
        assert_eq!(saved, 121);
    }

    #[derive(Clone)]
    struct CursorClient {
        pages: Rc<RefCell<std::collections::VecDeque<serde_json::Value>>>,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl SHNClient for CursorClient {
        fn new() -> Self {
            unreachable!()
        }
        fn post_request<RT: DeserializeOwned, JT: Serialize + ?Sized>(
            &self,
            _: &str,
            _: &JT,
            _: bool,
        ) -> Result<RT> {
            unreachable!()
        }

        fn get_request<RT: DeserializeOwned>(
            &self,
            path: &str,
            _: &str,
            parameters: Option<Vec<(&str, &str)>>,
            _: bool,
        ) -> Result<RT> {
            let body = if path.ends_with("/past_messages") {
                serde_json::json!({"messages": []})
            } else {
                let parameters = parameters.unwrap();
                let cursor = parameters.iter().find(|(key, _)| *key == "continuation");
                if let Some((_, cursor)) = cursor {
                    assert!(!parameters
                        .iter()
                        .any(|(key, _)| ["updated_from", "order", "count"].contains(key)));
                    self.calls.borrow_mut().push(cursor.to_string());
                } else {
                    assert!(self.calls.borrow().is_empty(), "lost the server cursor");
                    self.calls.borrow_mut().push("initial".into());
                }
                self.pages
                    .borrow_mut()
                    .pop_front()
                    .expect("requested beyond final cursor page")
            };
            Ok(serde_json::from_value(body).unwrap())
        }
    }

    fn cursor_crawl(pages: Vec<serde_json::Value>) -> (Result<()>, usize, Vec<String>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let directory = tempdir::TempDir::new("colmsg-cursor").unwrap();
        let config = Config {
            name: vec!["test".into()],
            from: None,
            to: None,
            kind: vec![Kind::Text],
            dir: directory.path().to_path_buf(),
            access_token: "test".into(),
            member_id: None,
            message_group_id: Some(1),
            client: CursorClient {
                pages: Rc::new(RefCell::new(pages.into())),
                calls: calls.clone(),
            },
        };
        let result = Saver::new(&config).save();
        let saved = fs::read_dir(directory.path().join("test")).unwrap().count();
        let requests = calls.borrow().clone();
        (result, saved, requests)
    }

    fn cursor_page(
        ids: std::ops::Range<u32>,
        continuation: Option<&str>,
        comments: usize,
    ) -> serde_json::Value {
        // Shared timestamps also ensure cursor paging takes precedence over the
        // old strategy of increasing count to resolve timestamp ties.
        let messages: Vec<_> = ids
            .map(|id| {
                serde_json::json!({
                    "id": id, "group_id": 1, "is_favorite": false, "is_silent": false,
                    "published_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                    "state": "published", "type": "text", "text": "test"
                })
            })
            .collect();
        serde_json::json!({"messages": messages, "letters": [],
            "comments": vec![serde_json::json!({}); comments],
            "queried_at": "2026-11-06T00:00:00Z", "continuation": continuation})
    }

    #[test]
    fn follows_continuation_through_mixed_and_message_empty_pages() {
        let (result, saved, calls) = cursor_crawl(vec![
            cursor_page(1..91, Some("opaque-next-1"), 10),
            cursor_page(0..0, Some("opaque-next-2"), 100),
            cursor_page(91..111, None, 0),
        ]);
        result.unwrap();
        assert_eq!(saved, 110);
        assert_eq!(calls, vec!["initial", "opaque-next-1", "opaque-next-2"]);
    }

    #[test]
    fn cursor_handles_full_pages_with_identical_timestamps() {
        let (result, saved, calls) = cursor_crawl(vec![
            cursor_page(1..101, Some("next-full-page"), 0),
            cursor_page(101..201, None, 0),
        ]);
        result.unwrap();
        assert_eq!(saved, 200);
        assert_eq!(calls, vec!["initial", "next-full-page"]);
    }

    #[test]
    fn repeated_continuation_is_an_error_not_success_or_an_infinite_loop() {
        let (result, _, calls) = cursor_crawl(vec![
            cursor_page(1..2, Some("repeated"), 0),
            cursor_page(2..3, Some("repeated"), 0),
        ]);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("repeated a continuation"));
        assert_eq!(calls.len(), 2);
    }
}
