use crate::order::LimitOrder;
use crate::order::OrderSide;
use std::collections::HashMap;

pub struct OrderBook {
    symbol: String,
    buy_orders: Vec<LimitOrder>,
    sell_orders: Vec<LimitOrder>,

    //For quick lookup/better cancellation
    index: HashMap<U256, (OrderSide, usize)>,
}

// Create instance representing an order book.
//
impl OrderBook {
    pub fn new(symbol: String) -> OrderBook {
        OrderBook {
            symbol,
            buy_orders: Vec::new(OrderSide::Buy),
            sell_orders: Vec::new(OrderSide::Sell),
            index: HashMap::new(),
        }
    }
    pub fn add_order(&mut self, order: LimitOrder) {}

    pub fn cancel_order(&mut self, order: LimitOrder) {}

    pub fn match_orders(&mut self) {}

    // Additional methods?
}
