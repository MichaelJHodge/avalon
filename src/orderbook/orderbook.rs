use crate::order::LimitOrder;
use crate::order::OrderSide;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::{BTreeMap, HashMap};

pub struct OrderBook {
    symbol: String,
    buy_orders: BTreeMap<U256, Vec<LimitOrder>>, // Orders are still organized by price
    sell_orders: BTreeMap<U256, Vec<LimitOrder>>,
    index: HashMap<U256, (OrderSide, U256)>, // Now using nonce for indexing: Nonce -> (Side, Price)
}
// Create instance representing an order book.

impl OrderBook {
    // Create a new order book instance
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            buy_orders: BTreeMap::new(),
            sell_orders: BTreeMap::new(),
            index: HashMap::new(),
        }
    }
    // Add or update orders in the book
    pub fn add_order(&mut self, order: LimitOrder) -> Result<(), &'static str> {
        // Check for duplicate orders based on nonce
        if self.index.contains_key(&order.nonce) {
            return Err("Duplicate order detected based on nonce");
        }

        // Basic validation for the order
        if order.amount.is_zero() {
            return Err("Order amount cannot be zero");
        }
        // Add the order to the appropriate list

        let order_list = match order.side {
            OrderSide::Buy => &mut self.buy_orders,
            OrderSide::Sell => &mut self.sell_orders,
        };

        // Add the order to the list

        order_list
            .entry(order.price)
            .or_insert_with(Vec::new)
            .push(order.clone());

        // Update the index with the new order
        self.index
            .insert(order.nonce, (order.side.clone(), order.price));

        Ok(())
    }

    // Efficiently cancel orders without full re-sorting
    pub fn cancel_order(&mut self, nonce: U256) -> Result<(), &'static str> {
        if let Some((side, price)) = self.index.remove(&nonce) {
            let orders = match side {
                OrderSide::Buy => &mut self.buy_orders,
                OrderSide::Sell => &mut self.sell_orders,
            };

            if let Some(orders_at_price) = orders.get_mut(&price) {
                if let Some(pos) = orders_at_price.iter().position(|o| o.nonce == nonce) {
                    orders_at_price.remove(pos);
                    return Ok(());
                }
            }
        }

        Err("Order not found")
    }

    // Improve match_order to efficiently handle partial fills and direct cancellations
    // Match a new order against the order book
    // Match a new order against the order book and return information about the match
    pub fn match_order(
        &mut self,
        mut new_order: LimitOrder,
    ) -> Result<Vec<LimitOrder>, &'static str> {
        // Check for duplicate orders based on nonce
        if self.index.contains_key(&new_order.nonce) {
            return Err("Duplicate order detected based on nonce");
        }

        // Basic validation for the new_order
        if new_order.amount.is_zero() {
            return Err("Order amount cannot be zero");
        }
        if new_order.price.is_zero() {
            return Err("Order price cannot be zero");
        }

        let mut matched_orders = Vec::new();
        let mut price_keys: Vec<U256> = matching_orders.keys().cloned().collect();

        // Match the new_order against the opposite side of the book
        // Use a BinaryHeap to store the opposite orders for efficient matching
        // The BinaryHeap will be used as a max-heap for buy orders and a min-heap for sell orders
        // This allows us to efficiently match orders based on price

        let (matching_orders, self_orders) = match new_order.side {
            OrderSide::Buy => (&mut self.sell_orders, &mut self.buy_orders),
            OrderSide::Sell => (&mut self.buy_orders, &mut self.sell_orders),
        };

        if new_order.side == OrderSide::Sell {
            price_keys.sort();
        } else {
            price_keys.sort_by(|a, b| b.cmp(a)); // Reverse for buy orders to match with the highest price first
        }

        for price in price_keys.iter() {
            if !is_match_condition_met(new_order.side, *price, new_order.price)
                || new_order.amount.is_zero()
            {
                break;
            }

            if let Some(orders_at_price) = matching_orders.get_mut(price) {
                let mut i = 0;
                while i < orders_at_price.len() && !new_order.amount.is_zero() {
                    let order = &mut orders_at_price[i];

                    let trade_amount = std::cmp::min(new_order.amount, order.amount);
                    new_order.amount -= trade_amount;
                    order.amount -= trade_amount;

                    if order.amount.is_zero() {
                        matched_orders.push(orders_at_price.remove(i)); // Remove and push to matched_orders
                    } else {
                        order.status = OrderStatus::PartiallyFilled;
                        matched_orders.push(order.clone()); // Clone and push to matched_orders
                        i += 1; // Move to next order
                    }
                }
            }

            if new_order.amount.is_zero() {
                break; // Exit if the new order is fully matched
            }
        }

        // Update the status of the new order based on the remaining amount
        if !new_order.amount.is_zero() {
            new_order.status = OrderStatus::PartiallyFilled;
            self_orders
                .entry(new_order.price)
                .or_insert_with(Vec::new)
                .push(new_order);
        } else {
            new_order.status = OrderStatus::Filled;
        }

        Ok(matched_orders)
    }
    // Additional methods?
    fn is_match_condition_met(side: OrderSide, order_price: U256, new_order_price: U256) -> bool {
        match side {
            OrderSide::Buy => new_order_price >= order_price,
            OrderSide::Sell => new_order_price <= order_price,
        }
    }
}
