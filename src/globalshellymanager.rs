use crate::{ShellyDiscoveryResult, ShellyManager};
use futures::{stream::FuturesUnordered, StreamExt};
use std::error::Error;
use std::time::Duration;

pub struct GlobalShellyManager {
    pub shelly_list: Vec<ShellyManager>,
}

impl GlobalShellyManager {
    pub async fn new() -> GlobalShellyManager {
        GlobalShellyManager {
            shelly_list: vec![],
        }
    }

    pub async fn insert_shelly(
        &mut self,
        shelly_disc_result: ShellyDiscoveryResult,
        user_login: String,
        user_password: String,
    ) {
        for shelly in self.shelly_list.iter() {
            if shelly.mac_address == shelly_disc_result.mac_address
                && shelly.ip == shelly_disc_result.ip_address
            {
                return;
            }
        }

        let shelly_m = ShellyManager::new(
            &shelly_disc_result.ip_address,
            &shelly_disc_result.topic_name,
            &shelly_disc_result.mac_address,
            &shelly_disc_result.mdns_name,
            &user_login,
            &user_password,
        )
        .await;

        if let Ok(mut shelly) = shelly_m {
            shelly.send_get_update().await;
            self.shelly_list.push(shelly);
            println!(
                "Shelly {} {} connected",
                shelly_disc_result.topic_name, shelly_disc_result.mac_address
            );
        }
    }

    pub async fn send_ping(&mut self) {
        for shelly in self.shelly_list.iter_mut() {
            shelly.send_ping().await;
        }
    }

    pub async fn send_action(
        &mut self,
        mac_address: &str,
        action_payload: &serde_json::Value,
    ) -> Result<String, Box<dyn Error>> {
        for shelly in self.shelly_list.iter_mut() {
            if shelly.mac_address == mac_address {
                shelly.send_action(action_payload).await;
                //println!("DOMO: SHELLY_ACTION_SENT");
            }
        }

        Err("shelly not found".into())
    }

    pub async fn sleep_long(&self) -> Result<serde_json::Value, Box<dyn Error>> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Err("sleep long".into())
    }

    pub async fn wait_for_shelly_message(&mut self) -> Result<serde_json::Value, Box<dyn Error>> {
        let mut mac_address: String = String::from("");

        if self.shelly_list.is_empty() {
            let ret = self.sleep_long().await;
            return ret;
        } else {
            let mut futures = FuturesUnordered::new();
            for shelly in self.shelly_list.iter_mut() {
                futures.push(shelly.wait_for_shelly_message());
            }

            let res = futures.next().await;

            if let Some(res) = res {
                match res {
                    Ok(r) => {
                        return Ok(r);
                    }
                    Err(e) => {
                        let s = e.to_string();
                        if s.contains(':') {
                            mac_address = e.to_string();
                        }
                    }
                }
            }
        }

        if !mac_address.is_empty() {
            let mut to_remove: i32 = -1;
            for (idx, shelly) in self.shelly_list.iter_mut().enumerate() {
                if shelly.mac_address == mac_address {
                    to_remove = idx as i32;
                    break;
                }
            }
            if to_remove != -1 {
                self.shelly_list.remove(to_remove as usize);
            }
        }

        Err("no messages".into())
    }

    pub async fn check_if_reconnect_needed(&mut self) {
        let mut idx = 0_usize;

        while idx < self.shelly_list.len() {
            let elapsed = self.shelly_list[idx]
                .last_pong_timestamp
                .elapsed()
                .unwrap()
                .as_secs();

            println!(
                "{} pong_timestamp_elapsed {}",
                self.shelly_list[idx].mac_address, elapsed
            );
            if elapsed > 120 {
                println!("Reconnect to shelly {} ", self.shelly_list[idx].mac_address);
                let ret = self.shelly_list[idx].reconnect().await;
                println!("AFTER RECONNECT");
                match ret {
                    Ok(_) => {}
                    Err(_) => {
                        println!("Error while reconnecting");
                        self.shelly_list.remove(idx);
                        continue;
                    }
                }
            }

            idx += 1;
        }
    }
}
