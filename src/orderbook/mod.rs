use ethers::types::{Address, U256};
use libp2p::PeerId;
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
    user_id: PeerId,
    timestamp: i64,
    nonce: U256, // New field for uniqueness
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

    pub fn side(&self) -> Option<OrderSide> {
        match self.side {
            OrderSide::Buy => Some(OrderSide::Buy),
            OrderSide::Sell => Some(OrderSide::Sell),
            _ => None,
        }
    }
}
