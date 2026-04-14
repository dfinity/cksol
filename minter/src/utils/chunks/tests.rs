use super::*;

#[test]
fn should_split_into_chunks_of_given_size() {
    let data: Vec<i32> = (1..=7).collect();
    let chunks = data.iter().into_chunks(3).take_chunks(10);
    assert_eq!(chunks, vec![vec![1, 2, 3], vec![4, 5, 6], vec![7]]);
}

#[test]
fn should_limit_to_max_chunks() {
    let data: Vec<i32> = (1..=9).collect();
    let chunks = data.iter().into_chunks(3).take_chunks(2);
    assert_eq!(chunks, vec![vec![1, 2, 3], vec![4, 5, 6]]);
}

#[test]
fn should_return_empty_for_empty_input() {
    let data: Vec<i32> = vec![];
    let chunks = data.iter().into_chunks(3).take_chunks(5);
    assert!(chunks.is_empty());
}

#[test]
fn should_return_empty_when_max_chunks_is_zero() {
    let data: Vec<i32> = (1..=9).collect();
    let chunks = data.iter().into_chunks(3).take_chunks(0);
    assert!(chunks.is_empty());
}

#[test]
fn should_handle_chunk_size_larger_than_input() {
    let data: Vec<i32> = vec![1, 2, 3];
    let chunks = data.iter().into_chunks(10).take_chunks(5);
    assert_eq!(chunks, vec![vec![1, 2, 3]]);
}

#[test]
#[should_panic(expected = "chunk_size must be greater than zero")]
fn should_panic_on_zero_chunk_size() {
    let data: Vec<i32> = vec![1, 2, 3];
    let _ = data.iter().into_chunks(0);
}
