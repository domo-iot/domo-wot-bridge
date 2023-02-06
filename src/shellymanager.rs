use base64::encode;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::error::Error;
use std::time::SystemTime;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::{http, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub struct ShellyManager {
    pub ip: String,
    pub mac_address: String,
    pub url: String,
    pub write_shelly: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    pub read_shelly: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    pub last_pong_timestamp: std::time::SystemTime,
    pub last_action_timestamp: std::time::SystemTime,
    pub user_login: String,
    pub user_password: String,
}

impl ShellyManager {
    pub async fn connect_to_shelly(
        ip: &str,
        url: &str,
        user_login: &str,
        user_password: &str,
    ) -> Result<
        (
            SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
            SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        ),
        Box<dyn Error>,
    > {
        let _url_shelly = url::Url::parse(url).unwrap();
        let mut connect_attempts_counter = 0;

        let enc = encode(user_login.to_owned() + ":" + user_password);
        let header = format!("Basic {enc}");

        loop {
            let ws_request = http::Request::builder()
                .method("GET")
                .header("Host", url)
                .header("Origin", url)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header(
                    "Sec-WebSocket-Key",
                    tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                )
                .header("Authorization", header.clone())
                .uri(url)
                .body(())?;

            //let connector = native_tls::TlsConnector::builder().danger_accept_invalid_hostnames(true).build().unwrap();

            let connector = native_tls::TlsConnector::builder().build()?;

            let shelly_tcp_stream = TcpStream::connect(ip.to_owned() + ":443").await?;

            tokio::select! {

                //ws_shelly_res = tokio_tungstenite::connect_async_tls_with_config(ws_request, None, Some(tokio_tungstenite::Connector::NativeTls(connector))) => {

                ws_shelly_res = tokio_tungstenite::client_async_tls_with_config(ws_request, shelly_tcp_stream, None, Some(tokio_tungstenite::Connector::NativeTls(connector))) => {
                    if let Ok(ws_sh) = ws_shelly_res {
                        let ws_shelly = ws_sh.0;
                        let (write_shelly, read_shelly) = ws_shelly.split();
                        return Ok((write_shelly, read_shelly));
                       }else {
                         connect_attempts_counter += 1;
                         println!("{ws_shelly_res:?}");
                         if connect_attempts_counter == 2 {
                            return Err("connect error".into());
                         }
                    }


                }

                _ = tokio::time::sleep(Duration::from_millis(10000)) => {
                       println!("Connect to shelly timeout");
                       connect_attempts_counter += 1;

                       if connect_attempts_counter == 2 {
                        return Err("connect error".into());
                       }
                }

            }
        }
    }

    pub async fn new(
        ip: &str,
        topic_name: &str,
        mac_address: &str,
        mdns_name: &str,
        user_login: &str,
        user_password: &str,
    ) -> Result<ShellyManager, Box<dyn Error>> {
        let mac = mac_address.replace(':', "");
        let url = "wss://".to_owned() + mdns_name + "/things/" + topic_name + "-" + &mac;

        let (write_shelly, read_shelly) =
            ShellyManager::connect_to_shelly(ip, &url, user_login, user_password).await?;

        Ok(ShellyManager {
            ip: ip.to_owned(),
            mac_address: mac_address.to_owned(),
            url: url.to_owned(),
            write_shelly,
            read_shelly,
            last_pong_timestamp: SystemTime::now(),
            last_action_timestamp: SystemTime::UNIX_EPOCH,
            user_login: user_login.to_owned(),
            user_password: user_password.to_owned(),
        })
    }

    pub async fn reconnect(&mut self) -> Result<(), Box<dyn Error>> {
        let (write_shelly, read_shelly) = ShellyManager::connect_to_shelly(
            &self.ip,
            &self.url,
            &self.user_login,
            &self.user_password,
        )
        .await?;
        self.last_pong_timestamp = SystemTime::now();
        self.last_action_timestamp = SystemTime::UNIX_EPOCH;
        self.write_shelly = write_shelly;
        self.read_shelly = read_shelly;
        Ok(())
    }

    pub async fn send_ping(&mut self) {
        let _ret = self.write_shelly.send(Message::Ping(vec![])).await;
        println!("ping message to shelly sent ");
    }

    pub async fn send_action(&mut self, message: &serde_json::Value) {
        if self.last_action_timestamp != SystemTime::UNIX_EPOCH
            && self.last_action_timestamp.elapsed().unwrap().as_millis() < 300
        {
            println!("Skipping action");
            return;
        }

        let _ret = self
            .write_shelly
            .send(Message::Text(message.to_string()))
            .await;
        println!("action message to shelly sent ");
        self.last_action_timestamp = SystemTime::now();
    }

    pub async fn wait_for_shelly_message(&mut self) -> Result<serde_json::Value, Box<dyn Error>> {
        let data = self.read_shelly.next().await;
        match data {
            Some(Ok(Message::Text(t))) => {
                let message: serde_json::Value = serde_json::from_str(&t)?;
                return Ok(message);
            }
            Some(Ok(Message::Pong(_t))) => {
                println!("Received Pong Message from shelly");
                self.last_pong_timestamp = SystemTime::now();
            }
            _ => {}
        }

        Err("not able to parse message".into())
    }
}
