use ethers::types::{Address, U256};
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt::Display;
use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum OrderSide {
    Buy,
    Sell,
}

// Convert OrderSide to a String or integer representation
pub fn order_side_to_db_value(side: &OrderSide) -> String {
    match side {
        OrderSide::Buy => "Buy".to_string(),
        OrderSide::Sell => "Sell".to_string(),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum OrderStatus {
    Open,
    Filled,
    PartiallyFilled,
    Cancelled,
}
// Implementing `Display` for `OrderStatus`
impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                OrderStatus::Open => "Open",
                OrderStatus::Filled => "Filled",
                OrderStatus::PartiallyFilled => "PartiallyFilled",
                OrderStatus::Cancelled => "Cancelled",
            }
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LimitOrder {
    pub side: OrderSide,
    pub asset: String,
    pub amount: U256,
    pub price: U256,
    pub status: OrderStatus,
    pub user_id: PeerId,
    pub timestamp: i64,
    pub nonce: U256, // New field for uniqueness
}

impl LimitOrder {
    pub fn new(
        side: OrderSide,
        asset: String,
        amount: U256,
        price: U256,
        user_id: PeerId,
        nonce: U256, // Added nonce as a parameter
    ) -> Self {
        LimitOrder {
            side,
            asset,
            amount,
            price,
            status: OrderStatus::Open,
            user_id,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            nonce,
        }
    }
}

impl Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "Buy"),
            OrderSide::Sell => write!(f, "Sell"),
        }
    }
}

// Define a comparator for LimitOrder that considers price and timestamp
impl Ord for LimitOrder {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .price
            .cmp(&self.price) // Higher price first
            .then_with(|| self.timestamp.cmp(&other.timestamp)) // Earlier timestamp first
    }
}

impl PartialOrd for LimitOrder {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for LimitOrder {}

impl PartialEq for LimitOrder {
    fn eq(&self, other: &Self) -> bool {
        self.price == other.price && self.timestamp == other.timestamp
    }
}
