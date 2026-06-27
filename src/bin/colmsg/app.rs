use std::fs;
use std::path::PathBuf;

use chrono::NaiveDateTime;
use clap::ArgMatches;
use wild;

use colmsg::{
    dirs::PROJECT_DIRS,
    errors::*,
    http::client::{AClient, HClient, MClient, NClient, SClient, SHNClient, YClient},
    Config, Kind,
};

use crate::{clap_app, config::get_access_token_from_file, config::get_args_from_config_file};

pub struct App {
    pub matches: ArgMatches<'static>,
}

impl App {
    pub fn new() -> Result<Self> {
        Ok(App {
            matches: Self::matches()?,
        })
    }

    fn matches() -> Result<ArgMatches<'static>> {
        let mut cli_args = wild::args_os();
        let mut args = get_args_from_config_file().expect("Could not parse configuration file");

        args.insert(0, cli_args.next().unwrap());
        cli_args.for_each(|string| args.push(string));

        Ok(clap_app::build_app().get_matches_from(args))
    }

    fn normalize_name(name: &str) -> String {
        name.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
    }

    fn parse_datetime_arg(value: Option<&str>) -> Result<Option<NaiveDateTime>> {
        match value {
            Some(value) => {
                let formats = [
                    "%Y-%m-%dT%H:%M:%SZ",
                    "%Y/%m/%d %H:%M:%S",
                    "%Y-%m-%d %H:%M:%S",
                ];

                for format in &formats {
                    if let Ok(datetime) = NaiveDateTime::parse_from_str(value, format) {
                        return Ok(Some(datetime));
                    }
                }

                Err(format!(
                    "Invalid date format: {}. Use YYYY-MM-DDTHH:MM:SSZ.",
                    value
                )
                .into())
            }
            None => Ok(None),
        }
    }

    fn normalize_access_token(access_token: &str) -> String {
        let mut token = access_token
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim();

        if token.to_ascii_lowercase().starts_with("authorization:") {
            token = token.splitn(2, ':').nth(1).unwrap_or("").trim();
        }

        if token.to_ascii_lowercase().starts_with("bearer ") {
            token = token[7..].trim();
        }

        token.to_string()
    }

    pub fn sakurazaka_config(&self) -> Result<Config<SClient>> {
        let client = SClient::new();
        self.config("s_refresh_token", "s_access_token", client)
    }

    pub fn hinatazaka_config(&self) -> Result<Config<HClient>> {
        let client = HClient::new();
        self.config("h_refresh_token", "h_access_token", client)
    }

    pub fn nogizaka_config(&self) -> Result<Config<NClient>> {
        let client = NClient::new();
        self.config("n_refresh_token", "n_access_token", client)
    }

    pub fn asukasaito_config(&self) -> Result<Config<AClient>> {
        let client = AClient::new();
        self.config("a_refresh_token", "a_access_token", client)
    }

    pub fn maishiraishi_config(&self) -> Result<Config<MClient>> {
        let client = MClient::new();
        self.config("m_refresh_token", "m_access_token", client)
    }

    pub fn yodel_config(&self) -> Result<Config<YClient>> {
        let client = YClient::new();
        self.config("y_refresh_token", "y_access_token", client)
    }

    fn config<S: AsRef<str>, A: AsRef<str>, C: SHNClient>(
        &self,
        refresh_token_str: S,
        access_token_str: A,
        client: C,
    ) -> Result<Config<C>> {
        let name = match self.matches.values_of("name") {
            Some(names) => names.map(Self::normalize_name).collect::<Vec<_>>(),
            None => vec![],
        };

        let from = Self::parse_datetime_arg(self.matches.value_of("from"))?;
        let to = Self::parse_datetime_arg(self.matches.value_of("to"))?;
        if let (Some(from), Some(to)) = (from.as_ref(), to.as_ref()) {
            if from > to {
                return Err("From date must be earlier than To date.".into());
            }
        }

        let kind = match self.matches.values_of("kind") {
            Some(k) => {
                k.map(|v| {
                    match v {
                        "text" => Kind::Text,
                        "picture" => Kind::Picture,
                        "video" => Kind::Video,
                        "voice" => Kind::Voice,
                        "link" => Kind::Link,
                        _ => Kind::Link, // _ はあり得ないはずだが怒られるのでとりあえずLinkにする
                    }
                })
                .collect::<Vec<_>>()
            }
            None => vec![
                Kind::Text,
                Kind::Picture,
                Kind::Video,
                Kind::Voice,
                Kind::Link,
            ],
        };

        let dir = self
            .matches
            .value_of("dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| PROJECT_DIRS.download_dir().to_path_buf());
        if !dir.is_dir() {
            println!("create download directory: {}", dir.display());
            if let Err(e) = fs::create_dir_all(&dir) {
                return Err(e.into());
            }
        }

        let member_id = self
            .matches
            .value_of("member_id")
            .map(|id| id.parse::<u32>())
            .transpose()
            .map_err(|e| format!("Invalid member id: {}", e))?;

        let message_group_id = self
            .matches
            .value_of("message_group_id")
            .map(|id| id.parse::<u32>())
            .transpose()
            .map_err(|e| format!("Invalid message group id: {}", e))?;

        let access_token = match self.matches.value_of(access_token_str.as_ref()) {
            Some(access_token) => Self::normalize_access_token(access_token),
            None => {
                let refresh_token = self
                    .matches
                    .value_of(refresh_token_str.as_ref())
                    .map(String::from)
                    .unwrap_or_else(|| String::from("invalid_refresh_token"));
                get_access_token_from_file(&refresh_token, client.clone())?
            }
        };
        Ok(Config {
            name,
            from,
            to,
            kind,
            dir,
            client: client.clone(),
            access_token,
            member_id,
            message_group_id,
        })
    }
}
