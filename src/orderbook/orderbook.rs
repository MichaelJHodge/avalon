use crate::order::LimitOrder;
use crate::order::OrderSide;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;

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

pub struct OrderBook {
    symbol: String,
    buy_orders: BinaryHeap<LimitOrder>,
    sell_orders: BinaryHeap<LimitOrder>,

    //For quick lookup/better cancellation
    index: HashMap<U256, (OrderSide, usize)>,
}

// Create instance representing an order book.

impl OrderBook {
    // Create a new order book instance
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            buy_orders: BinaryHeap::new(),
            sell_orders: BinaryHeap::new(),
            index: HashMap::new(),
        }
    }
    // Add or update orders in the book
    pub fn add_order(&mut self, order: LimitOrder) {
        let orders = match order.side {
            OrderSide::Buy => &mut self.buy_orders,
            OrderSide::Sell => &mut self.sell_orders,
        };

        // Insert the order into the heap based on its side
        orders.push(order.clone());
        // Update the index for quick lookup and cancellation
        self.index.insert(order.nonce, (order.side.clone(), order));
    }

    // Efficiently cancel orders without full re-sorting
    pub fn cancel_order(&mut self, nonce: &U256) -> Result<(), String> {
        if let Some((side, _)) = self.index.remove(nonce) {
            // Flag the order as cancelled in the index to avoid executing it during matching
            // Actual removal from the BinaryHeap is non-trivial without direct support
            // Orders flagged as cancelled can be skipped during the match_order process

            Ok(())
        } else {
            Err("Order not found".to_string())
        }
    }

    // Improve match_order to efficiently handle partial fills and direct cancellations
    pub fn match_order(&mut self, new_order: &LimitOrder) -> Vec<LimitOrder> {
        let (active_orders, opposite_orders) = match new_order.side {
            OrderSide::Buy => (&mut self.buy_orders, &mut self.sell_orders),
            OrderSide::Sell => (&mut self.sell_orders, &mut self.buy_orders),
        };

        // Placeholder for matched orders
        let mut matched_orders: Vec<LimitOrder> = Vec::new();

        // Simplified matching logic for demonstration
        while let Some(order) = opposite_orders.peek() {
            // Check for price conditions or if order is cancelled (skipping logic)
            // This is a simplified view; actual matching would consider quantities and partial fills

            // For a real implementation, you'd remove the matched order from the heap
            // and handle partial fills by adjusting quantities and potentially re-adding
            // the partially filled order back to the heap

            // Mock logic to push the matched order and break
            matched_orders.push(order.clone());
            break; // Replace with actual matching logic
        }

        matched_orders
    }
    // Additional methods?
}
