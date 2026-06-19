use serde::{Deserialize, Serialize};
use crate::{errors::*, http::client::SHNClient};

const PATH: &str = "/v2/members";

#[derive(Serialize, Deserialize, Debug)]
pub struct Member {
    pub id: u32,
    pub name: String,
    pub groups: Vec<u32>,
}

pub fn request<C: SHNClient>(client: C, access_token: &String, id: &u32) -> Result<Member> {
    let path = format!("{}/{}", PATH, id);
    let access_token = String::from(access_token);

    client.get_request::<Member>(path.as_str(), &access_token, None, false)
}
