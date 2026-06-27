use std::fs;
use std::path::PathBuf;

use chrono::NaiveDateTime;
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

        // 購読しているメンバーのメッセージを取得するAPIを複数回叩くためのループ
        loop {
            if self.is_from_after_to(&fromdate)? {
                break;
            }

            let timeline = http::timeline::request(
                self.config.client.clone(),
                &self.config.access_token,
                &member_identifier.id,
                &fromdate,
                &count.to_string(),
            )?;

            let message_length = timeline.messages.len();
            if message_length == 0 {
                break;
            }

            let reached_to = self.timeline_reached_to(&timeline)?;

            // updated_atの値を基準にメッセージを取得している
            // 取得したメッセージのupdated_atがすべて同じだと基準が判明しない
            // 最新のメッセージまで取得出来たか、異なるupdated_atの値が現れるまでメッセージ取得数を増やしてメッセージ取得を施行する
            if !reached_to && message_length >= count && self.are_all_updated_at_same(&timeline) {
                count += http::timeline::DEFAULT_COUNT;
                continue;
            }

            // メッセージを取得するAPIを叩くと複数件のメッセージを取得出来る
            // そのメッセージを1件ずつ処理するためのループ
            for message in &timeline.messages {
                self.save_message(&message, &id_dates, &member_dir_buf)?
            }
            id_dates = self.id_dates(&member_dir_buf);

            // 最新のメッセージまで保存し終わったら終了する
            if reached_to || message_length < http::timeline::DEFAULT_COUNT {
                break;
            };
            fromdate = self.latest_timeline_date(&timeline)?;

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
            .last()
            .ok_or_else(|| "timeline has no messages".to_string())?;
        Self::parse_message_updated_at(&message.updated_at)?;
        Ok(message.updated_at.clone())
    }

    fn create_member_dir_buf(&self, member_identifier: &MemberIdentifier) -> Result<PathBuf> {
        let mut member_dir_buf = self.config.dir.clone();
        member_dir_buf.push(&member_identifier.gen);
        member_dir_buf.push(&member_identifier.name);
        if !member_dir_buf.is_dir() {
            println!("create directory: {}", member_dir_buf.display());
            fs::create_dir_all(&member_dir_buf)?
        }
        Ok(member_dir_buf)
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
        result.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)).then(a.media.cmp(&b.media)));
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
