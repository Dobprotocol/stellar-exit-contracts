#![cfg(test)]
extern crate std;

use soroban_sdk::{testutils::Address as _, Address, Env};

use crate::{Error, FifoQueue, FifoQueueClient, MAX_QUEUE_DEPTH};

fn setup() -> (Env, FifoQueueClient<'static>, Address) {
    let e = Env::default();
    e.mock_all_auths();
    let admin = Address::generate(&e);
    let queue = FifoQueueClient::new(&e, &e.register(FifoQueue, ()));
    queue.initialize(&admin);
    queue.set_auction(&Address::generate(&e));
    let asset = Address::generate(&e);
    (e, queue, asset)
}

#[test]
fn the_line_is_first_in_first_out() {
    let (_e, queue, asset) = setup();

    assert_eq!(queue.enqueue(&asset, &1), 0);
    assert_eq!(queue.enqueue(&asset, &2), 1);
    assert_eq!(queue.enqueue(&asset, &3), 2);

    assert_eq!(queue.head(&asset), Some(1));
    assert_eq!(queue.depth(&asset), 3);
    assert_eq!(queue.position_of(&asset, &3), Some(2));
}

#[test]
fn leaving_the_line_moves_everyone_behind_up_by_exactly_one() {
    let (_e, queue, asset) = setup();
    queue.enqueue(&asset, &1);
    queue.enqueue(&asset, &2);
    queue.enqueue(&asset, &3);

    queue.dequeue(&asset, &2);

    assert_eq!(queue.position_of(&asset, &1), Some(0));
    assert_eq!(queue.position_of(&asset, &2), None);
    assert_eq!(queue.position_of(&asset, &3), Some(1));
    assert_eq!(queue.depth(&asset), 2);
}

#[test]
fn queues_are_per_asset_and_do_not_see_each_other() {
    let (e, queue, asset) = setup();
    let other = Address::generate(&e);

    queue.enqueue(&asset, &1);
    queue.enqueue(&other, &2);

    assert_eq!(queue.head(&asset), Some(1));
    assert_eq!(queue.head(&other), Some(2));
    assert_eq!(queue.depth(&asset), 1);
    assert_eq!(queue.position_of(&other, &1), None);
}

#[test]
fn an_exit_cannot_take_two_places_in_line() {
    let (_e, queue, asset) = setup();
    queue.enqueue(&asset, &1);
    assert_eq!(queue.try_enqueue(&asset, &1), Err(Ok(Error::AlreadyQueued)));
}

#[test]
fn dequeuing_something_that_never_queued_is_an_error() {
    let (_e, queue, asset) = setup();
    assert_eq!(queue.try_dequeue(&asset, &9), Err(Ok(Error::NotQueued)));
}

#[test]
fn the_queue_is_bounded() {
    let (_e, queue, asset) = setup();
    for id in 0..MAX_QUEUE_DEPTH as u64 {
        queue.enqueue(&asset, &id);
    }
    assert_eq!(queue.depth(&asset), MAX_QUEUE_DEPTH);
    assert_eq!(
        queue.try_enqueue(&asset, &999),
        Err(Ok(Error::QueueFull))
    );
}

#[test]
fn only_the_auction_moves_the_line() {
    let e = Env::default();
    let admin = Address::generate(&e);
    let queue = FifoQueueClient::new(&e, &e.register(FifoQueue, ()));
    e.mock_all_auths();
    queue.initialize(&admin);
    let asset = Address::generate(&e);

    // No auction wired yet: nothing can enter the line, admin included.
    assert_eq!(queue.try_enqueue(&asset, &1), Err(Ok(Error::NotAuction)));
}
