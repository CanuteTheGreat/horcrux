//! Network Zones Module
//!
//! Manages network zones - top-level containers for virtual networks

use super::{Zone, ZoneType};
use std::collections::HashMap;

pub struct ZoneManager {
    zones: HashMap<String, Zone>,
}

impl ZoneManager {
    pub fn new() -> Self {
        ZoneManager {
            zones: HashMap::new(),
        }
    }

    /// Create a new zone
    pub fn create(&mut self, zone: Zone) -> Result<(), String> {
        if self.zones.contains_key(&zone.id) {
            return Err(format!("Zone {} already exists", zone.id));
        }

        // Validate based on zone type
        match zone.zone_type {
            ZoneType::Simple => {
                // Simple zones don't require special validation
            }
            ZoneType::Vxlan => {
                if zone.nodes.is_empty() {
                    return Err("VXLAN zone requires at least one node".to_string());
                }
            }
            ZoneType::Evpn => {
                return Err("EVPN zones not yet supported".to_string());
            }
        }

        self.zones.insert(zone.id.clone(), zone);
        Ok(())
    }

    /// Get zone by ID
    pub fn get(&self, zone_id: &str) -> Option<&Zone> {
        self.zones.get(zone_id)
    }

    /// List all zones
    pub fn list(&self) -> Vec<&Zone> {
        self.zones.values().collect()
    }

    /// List zones of a specific type
    pub fn list_by_type(&self, zone_type: ZoneType) -> Vec<&Zone> {
        self.zones.values()
            .filter(|z| z.zone_type == zone_type)
            .collect()
    }

    /// List zones containing a specific node
    pub fn list_by_node(&self, node_id: &str) -> Vec<&Zone> {
        self.zones.values()
            .filter(|z| z.nodes.contains(&node_id.to_string()))
            .collect()
    }

    /// Update zone
    pub fn update(&mut self, zone: Zone) -> Result<(), String> {
        if !self.zones.contains_key(&zone.id) {
            return Err(format!("Zone {} not found", zone.id));
        }

        self.zones.insert(zone.id.clone(), zone);
        Ok(())
    }

    /// Delete zone
    pub fn delete(&mut self, zone_id: &str) -> Result<(), String> {
        if self.zones.remove(zone_id).is_none() {
            return Err(format!("Zone {} not found", zone_id));
        }
        Ok(())
    }

    /// Add node to zone
    pub fn add_node(&mut self, zone_id: &str, node_id: String) -> Result<(), String> {
        let zone = self.zones.get_mut(zone_id)
            .ok_or_else(|| format!("Zone {} not found", zone_id))?;

        if !zone.nodes.contains(&node_id) {
            zone.nodes.push(node_id);
        }

        Ok(())
    }

    /// Remove node from zone
    pub fn remove_node(&mut self, zone_id: &str, node_id: &str) -> Result<(), String> {
        let zone = self.zones.get_mut(zone_id)
            .ok_or_else(|| format!("Zone {} not found", zone_id))?;

        zone.nodes.retain(|n| n != node_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_zone() {
        let mut zm = ZoneManager::new();

        let zone = Zone {
            id: "zone1".to_string(),
            name: "Test Zone".to_string(),
            zone_type: ZoneType::Simple,
            description: "Test".to_string(),
            nodes: vec![],
        };

        assert!(zm.create(zone).is_ok());
        assert_eq!(zm.list().len(), 1);
    }

    #[test]
    fn test_add_remove_node() {
        let mut zm = ZoneManager::new();

        let zone = Zone {
            id: "zone1".to_string(),
            name: "Test".to_string(),
            zone_type: ZoneType::Simple,
            description: "".to_string(),
            nodes: vec![],
        };

        zm.create(zone).unwrap();
        assert!(zm.add_node("zone1", "node1".to_string()).is_ok());
        assert_eq!(zm.get("zone1").unwrap().nodes.len(), 1);

        assert!(zm.remove_node("zone1", "node1").is_ok());
        assert_eq!(zm.get("zone1").unwrap().nodes.len(), 0);
    }
}
