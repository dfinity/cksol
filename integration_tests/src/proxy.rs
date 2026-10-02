use candid::{CandidType, Decode, Deserialize, Encode, Principal, utils::ArgumentEncoder};

#[derive(CandidType)]
struct ProxyArgs {
    canister_id: Principal,
    method: String,
    args: Vec<u8>,
    cycles: u128,
}

#[derive(CandidType, Deserialize)]
struct ProxySucceed {
    result: Vec<u8>,
}

#[derive(CandidType, Deserialize, Debug)]
enum ProxyError {
    InsufficientCycles { available: u128, required: u128 },
    CallFailed { reason: String },
    UnauthorizedUser,
}

pub fn encode_call<In: ArgumentEncoder>(
    canister_id: Principal,
    method: &str,
    args: In,
    cycles: u128,
) -> Vec<u8> {
    let args = candid::encode_args(args).expect("Failed to encode arguments");
    Encode!(&ProxyArgs {
        canister_id,
        method: method.to_string(),
        args,
        cycles,
    })
    .expect("Failed to encode proxy arguments")
}

pub fn unwrap_reply(reply: Vec<u8>) -> Vec<u8> {
    match Decode!(&reply, Result<ProxySucceed, ProxyError>)
        .expect("Failed to decode proxy response")
    {
        Ok(ProxySucceed { result }) => result,
        Err(error) => panic!("Proxy call failed: {error:?}"),
    }
}
