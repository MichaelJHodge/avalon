use ethers::types::{Address, U256};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum OrderStatus {
    Open,
    Filled,
    PartiallyFilled,
    Cancelled,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LimitOrder {
    side: OrderSide,
    asset: String,
    amount: U256,
    price: U256,
    status: OrderStatus,
    user_id: Address,
    timestamp: i64,
    nonce: U256, // New field for uniqueness
}

impl LimitOrder {
    pub fn new(
        side: OrderSide,
        asset: String,
        amount: U256,
        price: U256,
        user_id: Address,
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

    pub fn side(&self) -> Option<OrderSide> {
        match self.side {
            0 => Some(OrderSide::Buy),
            1 => Some(OrderSide::Sell),
            _ => None,
        }
    }
}
