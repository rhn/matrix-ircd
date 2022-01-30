// Copyright 2016 Openmarket
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The main module responsible for keeping track of the Matrix side of the world.
//!
//! It knows nothing about IRC.

use futures::stream::Stream;
use futures::{Async, Future, Poll};

use std::collections::BTreeMap;
use std::io;

use tokio_core::reactor::Handle;

use url::percent_encoding::{percent_encode, PATH_SEGMENT_ENCODE_SET};
use url::Url;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json;

use crate::http::{HttpClient, Request};

use rand::{thread_rng, Rng};

use regex::Regex;

use quick_error::quick_error;

mod models;
pub mod protocol;
mod sync;

pub use self::models::{Member, Room};

/// A single Matrix session.
///
/// A `MatrixClient` both send requests and outputs a Stream of `SyncResponse`'s. It also keeps track
/// of vaious
pub struct MatrixClient {
    url: Url,
    user_id: String,
    access_token: String,
    sync_client: sync::MatrixSyncClient,
    rooms: BTreeMap<String, Room>,
    http_stream: HttpClient,
}

impl MatrixClient {
    pub fn new(
        handle: Handle,
        http_stream: HttpClient,
        base_url: &Url,
        user_id: String,
        access_token: String,
    ) -> MatrixClient {
        MatrixClient {
            url: base_url.clone(),
            user_id,
            access_token: access_token.clone(),
            sync_client: sync::MatrixSyncClient::new(handle, base_url, access_token),
            rooms: BTreeMap::new(),
            http_stream,
        }
    }

    /// The user ID associated with this session.
    pub fn get_user_id(&self) -> &str {
        &self.user_id
    }

    /// Create a session by logging in with a user name and password.
    pub fn login(
        handle: Handle,
        base_url: Url,
        user: String,
        password: String,
    ) -> Box<dyn Future<Item = MatrixClient, Error = LoginError>> {
        let host = base_url
            .host_str()
            .expect("expected host in base_url")
            .to_string();
        let port = base_url.port_or_known_default().unwrap();
        let tls = match base_url.scheme() {
            "http" => false,
            "https" => true,
            _ => panic!("Unrecognized scheme {}", base_url.scheme()),
        };

        let mut http_stream = HttpClient::new(host, port, tls, handle.clone());

        let f = do_json_post(
            "POST",
            &mut http_stream,
            &base_url.join("/_matrix/client/r0/login").unwrap(),
            &protocol::LoginPasswordInput {
                user,
                password,
                login_type: "m.login.password".into(),
            },
        )
        .map_err(move |err| match err {
            JsonPostError::Io(e) => LoginError::Io(e),
            JsonPostError::ErrorRepsonse(code, error) if code == 401 || code == 403 => {
                LoginError::InvalidPassword
            }
            JsonPostError::ErrorRepsonse(code, error) => LoginError::Io(io::Error::new(
                io::ErrorKind::Other,
                format!("Got {} response {:?}", code, error),
            )),
        })
        .map(move |response: protocol::LoginResponse| {
            MatrixClient::new(
                handle,
                http_stream,
                &base_url,
                response.user_id,
                response.access_token,
            )
        });

        Box::new(f)
    }

    pub fn send_text_message(
        &mut self,
        room_id: &str,
        body: String,
        msgtype: String,
    ) -> Box<dyn Future<Item = protocol::RoomSendResponse, Error = io::Error>> {
        let msg_id = thread_rng().gen::<u16>();
        let mut url = self
            .url
            .join(&format!(
                "/_matrix/client/r0/rooms/{}/send/m.room.message/mircd-{}",
                room_id, msg_id
            ))
            .expect("Unable to construct a valid API url");
        url.query_pairs_mut()
            .clear()
            .append_pair("access_token", &self.access_token);

        let f = do_json_post(
            "PUT",
            &mut self.http_stream,
            &url,
            &protocol::RoomSendInput { body, msgtype },
        )
        .map_err(JsonPostError::into_io_error);

        Box::new(f)
    }

    pub fn join_room(
        &mut self,
        room_id: &str,
    ) -> Box<dyn Future<Item = protocol::RoomJoinResponse, Error = io::Error>> {
        let roomid_encoded = percent_encode(room_id.as_bytes(), PATH_SEGMENT_ENCODE_SET);
        let mut url = self
            .url
            .join(&format!("/_matrix/client/r0/join/{}", roomid_encoded))
            .expect("Unable to construct a valid API url");

        url.query_pairs_mut()
            .clear()
            .append_pair("access_token", &self.access_token);

        let f = do_json_post(
            "POST",
            &mut self.http_stream,
            &url,
            &protocol::RoomJoinInput {},
        )
        .map_err(JsonPostError::into_io_error);

        Box::new(f)
    }

    pub fn get_room(&self, room_id: &str) -> Option<&Room> {
        self.rooms.get(room_id)
    }

    pub fn media_url(&self, url: &str) -> String {
        lazy_static::lazy_static! {
            static ref RE: Regex = Regex::new("^mxc://([^/]+/[^/]+)$").unwrap();
        }
        if let Some(captures) = RE.captures(url) {
            self.url
                .join("/_matrix/media/v1/download/")
                .unwrap()
                .join(&captures[1])
                .unwrap()
                .into_string()
        } else {
            url.to_owned()
        }
    }

    fn poll_sync(&mut self) -> Poll<Option<protocol::SyncResponse>, io::Error> {
        let mut resp = futures::try_ready!(self.sync_client.poll());
        if let Some(ref mut sync_response) = resp {
            if let Some(rooms) = &mut sync_response.rooms {
                for (room_id, ref mut sync) in &mut rooms.join {
                    sync.timeline.events.retain(|ev| {
                        !ev.unsigned
                            .transaction_id
                            .as_ref()
                            .map(|txn_id| txn_id.starts_with("mircd-"))
                            .unwrap_or(false)
                    });

                    if let Some(room) = self.rooms.get_mut(room_id) {
                        room.update_from_sync(sync);
                        continue;
                    }

                    // We can't put this in an else because of the mutable borrow in the if condition.
                    self.rooms
                        .insert(room_id.clone(), Room::from_sync(room_id.clone(), sync));
                }
            }
        }

        Ok(Async::Ready(resp))
    }
}

fn do_json_post<I: Serialize, O: DeserializeOwned + 'static>(
    method: &'static str,
    stream: &mut HttpClient,
    url: &Url,
    input: &I,
) -> Box<dyn Future<Item = O, Error = JsonPostError>> {
    let f = stream
        .send_request(Request {
            method,
            path: format!("{}?{}", url.path(), url.query().unwrap_or("")),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: serde_json::to_vec(input).expect("input to be valid json"),
        })
        .map_err(|e| e.into())
        .and_then(move |resp| {
            if resp.code != 200 {
                return Err(JsonPostError::ErrorRepsonse(
                    resp.code,
                    serde_json::from_slice(&resp.data).map_err(|_| {
                       JsonPostError::from(io::Error::new(io::ErrorKind::InvalidData, "invalid error response"))
                    })?,
                ));
            }

            serde_json::from_slice(&resp.data).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid json response").into()
            })
        });

    Box::new(f)
}

quick_error! {
    #[derive(Debug)]
    enum JsonPostError {
        Io(err: io::Error) {
            from()
            description("io error")
            display("I/O error: {}", err)
            cause(err)
        }
        ErrorRepsonse(code: u16, error: protocol::ErrorResponse) {
            description("received non 200 response")
            display("Received response: {} {:?}", code, error)
        }
    }
}

impl JsonPostError {
    pub fn into_io_error(self) -> io::Error {
        match self {
            JsonPostError::Io(err) => err,
            JsonPostError::ErrorRepsonse(code, error) => {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!("Received {} response: {:?}", code, error),
                )
            }
        }
    }
}

quick_error! {
    #[derive(Debug)]
    pub enum LoginError {
        Io(err: io::Error) {
            from()
            description("io error")
            display("I/O error: {}", err)
            cause(err)
        }
        InvalidPassword {
            description("password was invalid")
            display("Password is invalid")
        }
    }
}

impl Stream for MatrixClient {
    type Item = protocol::SyncResponse;
    type Error = io::Error;

    fn poll(&mut self) -> Poll<Option<protocol::SyncResponse>, io::Error> {
        task_trace!("Polled matrix client");
        self.poll_sync()
    }
}

#[cfg(test)]
mod tests {
    use super::MatrixClient;
    use mockito::{mock, Matcher, Matcher::UrlEncoded};

    #[test]
    fn matrix_login() {
        let base_url = mockito::server_url().as_str().parse::<url::Url>().unwrap();
        let username = "sample_username".to_string();
        let password = "sample_password".to_string();
        let mut core = tokio_core::reactor::Core::new().expect("could not create a tokio core");
        let handle = core.handle();

        let mock_req = mock("POST", "/_matrix/client/r0/login?")
            .with_header("content-type", "application/json")
            .with_status(200)
            .create();

        // run the future to completion. The future will error since invalid json is
        // returned, but as long as the call is correct the error is outside the scope of this
        // test. It is explicitly handled here in case the mock assert panics.
        if let Err(e) = core.run(MatrixClient::login(
            handle.clone(),
            base_url,
            username,
            password,
        )) {
            println!("MatrixSyncClient returned an error: {:?}", e)
        }

        mock_req.assert();
    }

    #[test]
    fn send_text_message() {
        let base_url = mockito::server_url().as_str().parse::<url::Url>().unwrap();
        let user_id = "sample_user_id".to_string();
        let token = "sample_token".to_string();
        let room_id = "room_id";
        let mut core = tokio_core::reactor::Core::new().expect("could not create a tokio core");
        let handle = core.handle();

        let mock_req = mock(
            "PUT",
            Matcher::Regex(
                r"^/_matrix/client/r0/rooms/(.+)/send/m.room.message/mircd-(\d+)$".to_string(),
            ),
        )
        .match_query(UrlEncoded("access_token".to_string(), token.clone()))
        .with_status(200)
        .create();

        let httpclient = crate::http::HttpClient::new(
            base_url.host_str().unwrap().to_owned(),
            base_url.port().unwrap(),
            false,
            handle.clone(),
        );

        let mut client = MatrixClient::new(handle.clone(), httpclient, &base_url, user_id, token);

        // run the future to completion. The future will error since invalid json is
        // returned, but as long as the call is correct the error is outside the scope of this
        // test. It is explicitly handled here in case the mock assert panics.
        if let Err(e) = core.run(client.send_text_message(
            room_id,
            "sample_body".to_string(),
            "m.text".to_string(),
        )) {
            println!("MatrixSyncClient returned an error: {:?}", e)
        }

        // give the executor some time to execute the http request on a thread pool
        std::thread::sleep(std::time::Duration::from_millis(200));

        mock_req.assert();
    }
}
