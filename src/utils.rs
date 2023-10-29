use std::collections::HashMap;
use std::time::SystemTime;

#[derive(Clone)]
pub struct ValveData {
    pub desired_state: serde_json::Value,
    pub attempts: usize,
}

pub struct BestActuatorData {
    actuator_mac_address: String,
    rssi: i64,
    timestamp: std::time::SystemTime,
}

pub struct ValveCommandManager {
    pub valve_commands: HashMap<String, ValveData>,
    pub best_actuator: HashMap<String, BestActuatorData>,
}

impl ValveCommandManager {
    pub fn new() -> Self {
        ValveCommandManager {
            best_actuator: HashMap::new(),
            valve_commands: HashMap::new(),
        }
    }

    pub fn remove(&mut self, valve_mac_address: &str) {
        self.valve_commands.remove(valve_mac_address);
    }

    pub fn insert(&mut self, valve_mac_address: &str, valve_data: ValveData) {
        self.valve_commands
            .insert(valve_mac_address.to_owned(), valve_data);
    }

    pub fn update_best_actuator(
        &mut self,
        valve_mac_address: &str,
        actuator_mac_address: &str,
        rssi: i64,
    ) {
        let now = SystemTime::now();
        if self.best_actuator.contains_key(valve_mac_address) {
            let data = self.best_actuator.get(valve_mac_address).unwrap();
            if data.rssi < rssi || (data.timestamp.elapsed().unwrap().as_secs() > 30) {
                self.best_actuator.insert(
                    valve_mac_address.to_string(),
                    BestActuatorData {
                        actuator_mac_address: actuator_mac_address.to_string(),
                        rssi,
                        timestamp: now,
                    },
                );
            }
        } else {
            self.best_actuator.insert(
                valve_mac_address.to_string(),
                BestActuatorData {
                    actuator_mac_address: actuator_mac_address.to_string(),
                    rssi,
                    timestamp: now,
                },
            );
        }
    }

    pub fn get_best_actuator_for_valve(&self, valve_mac_address: &str) -> Option<String> {
        if self.best_actuator.contains_key(valve_mac_address) {
            let act = self.best_actuator.get(valve_mac_address).unwrap();
            Some(act.actuator_mac_address.clone())
        } else {
            None
        }
    }
}
