use std::ops::Mul;

#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    coins, to_binary, BankMsg, Binary, Decimal, Deps, DepsMut, Env, MessageInfo, Order, Reply,
    Response, StdError, StdResult, SubMsg, WasmMsg,
};
use cw2::set_contract_version;
use cw_storage_plus::Bound;
use cw_utils::parse_reply_instantiate_data;

use archway_bindings::{ArchwayMsg, ArchwayQuery, ArchwayResult};

use pantheon_utils::Share;

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{Config, CONFIG, SHARES};

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:pantheon-splitter";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

const INSTANTIATE_REPLY_ID: u64 = 1;

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut<ArchwayQuery>,
    _env: Env,
    info: MessageInfo,
    msg: InstantiateMsg,
) -> ArchwayResult<ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // Save the sender as the admin
    let config = Config {
        admin: info.sender.clone(),
        mutable: msg.mutable,
    };
    CONFIG.save(deps.storage, &config)?;

    check_share_percentages(&msg.shares)?;

    // Processing each share
    for share in msg.shares {
        // Validating the recipient address
        let recipient = deps.api.addr_validate(&share.recipient)?;

        // Saving the share
        SHARES.save(deps.storage, recipient, &share)?;
    }

    Ok(Response::new().add_attribute("admin", info.sender))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut<ArchwayQuery>,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> ArchwayResult<ContractError> {
    match msg {
        ExecuteMsg::UpdateShares { shares } => execute_update_shares(deps, env, info, shares),
        ExecuteMsg::AddCustomContract { code_id, msg } => {
            execute_add_custom_contract(deps, env, info, code_id, msg)
        }
        ExecuteMsg::UpdateCustomContractRewardMetadata {
            address,
            owner_address,
            rewards_address,
        } => execute_update_custom_contract_reward_metadata(
            deps,
            env,
            info,
            address,
            owner_address,
            rewards_address,
        ),
        ExecuteMsg::LockContract {} => execute_lock_contract(deps, env, info),
        ExecuteMsg::WithdrawRewards {} => execute_withdraw_rewards(deps, env, info),
        ExecuteMsg::DistributeNativeTokens {} => execute_distribute_native_tokens(deps, env, info),
    }
}

fn execute_update_shares(
    deps: DepsMut<ArchwayQuery>,
    _env: Env,
    info: MessageInfo,
    shares: Vec<Share>,
) -> ArchwayResult<ContractError> {
    let config = CONFIG.load(deps.storage)?;

    // Only mutable contracts can add a share
    if config.mutable == false {
        return Err(ContractError::ContractNotMutable {});
    }

    // Only the admin can add a share
    if info.sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }

    check_share_percentages(&shares)?;

    // Clearing the existing shares
    SHARES.clear(deps.storage);

    // Processing each share
    for share in shares {
        // Validating the recipient address
        let recipient = deps.api.addr_validate(&share.recipient)?;

        // Saving the share
        SHARES.save(deps.storage, recipient, &share)?;
    }

    Ok(Response::new())
}

fn execute_add_custom_contract(
    deps: DepsMut<ArchwayQuery>,
    env: Env,
    info: MessageInfo,
    code_id: u64,
    msg: Binary,
) -> ArchwayResult<ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if config.mutable == false {
        return Err(ContractError::ContractNotMutable {});
    }

    if info.sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }

    let msg: SubMsg<ArchwayMsg> = SubMsg::reply_on_success(
        WasmMsg::Instantiate {
            admin: Some(env.contract.address.to_string()),
            code_id,
            msg,
            funds: info.funds,
            label: "Pantheon Custom Contract".to_string(),
        },
        INSTANTIATE_REPLY_ID,
    );

    Ok(Response::new().add_submessage(msg))
}

fn execute_update_custom_contract_reward_metadata(
    deps: DepsMut<ArchwayQuery>,
    _env: Env,
    info: MessageInfo,
    address: String,
    owner_address: Option<String>,
    rewards_address: Option<String>,
) -> ArchwayResult<ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if config.mutable == false {
        return Err(ContractError::ContractNotMutable {});
    }

    if info.sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }

    let msg = ArchwayMsg::UpdateContractMetadata {
        contract_address: Some(address),
        owner_address,
        rewards_address,
    };

    Ok(Response::new().add_message(msg))
}

fn execute_lock_contract(
    deps: DepsMut<ArchwayQuery>,
    _env: Env,
    info: MessageInfo,
) -> ArchwayResult<ContractError> {
    let mut config = CONFIG.load(deps.storage)?;

    // Only the admin can lock the contract
    if info.sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }

    // Updating the contract to be immutable
    config.mutable = false;
    CONFIG.save(deps.storage, &config)?;

    Ok(Response::new())
}

fn execute_withdraw_rewards(
    deps: DepsMut<ArchwayQuery>,
    _env: Env,
    info: MessageInfo,
) -> ArchwayResult<ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }

    let msg = ArchwayMsg::WithdrawRewards {
        records_limit: Some(0),
        record_ids: vec![],
    };

    Ok(Response::new().add_message(msg))
}

fn execute_distribute_native_tokens(
    deps: DepsMut<ArchwayQuery>,
    env: Env,
    info: MessageInfo,
) -> ArchwayResult<ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if info.sender != config.admin {
        return Err(ContractError::Unauthorized {});
    }

    let mut msgs: Vec<BankMsg> = vec![];

    // Get the contract's native ARCH balance
    let balance = deps.querier.query_balance(env.contract.address, "aconst")?;

    // Get the total share percentage
    let shares = SHARES
        .range(deps.storage, None, None, Order::Ascending)
        .map(|item| {
            let (_, share) = item?;
            Ok(share)
        })
        .collect::<Result<Vec<Share>, ContractError>>()?;

    // Calculate the amount of rewards to send to each recipient
    for share in shares {
        let amount = balance.amount.mul(share.percentage);

        // Create bank messages to send rewards to each recipient
        msgs.push(BankMsg::Send {
            to_address: share.recipient.to_string(),
            amount: coins(amount.u128(), "aconst"),
        });
    }

    Ok(Response::new().add_messages(msgs))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn reply(deps: DepsMut<ArchwayQuery>, env: Env, msg: Reply) -> ArchwayResult<ContractError> {
    let config = CONFIG.load(deps.storage)?;

    if msg.id != INSTANTIATE_REPLY_ID {
        return Err(ContractError::InstantiateError {});
    };

    let reply = parse_reply_instantiate_data(msg);

    match reply {
        Ok(res) => {
            let update_contract_metadata_msg = ArchwayMsg::UpdateContractMetadata {
                contract_address: Some(res.contract_address.clone()),
                owner_address: Some(env.contract.address.to_string()),
                rewards_address: Some(env.contract.address.to_string()),
            };
            let update_admin_msg = WasmMsg::UpdateAdmin {
                contract_addr: res.contract_address.clone(),
                admin: config.admin.to_string(),
            };

            Ok(Response::new()
                .add_message(update_contract_metadata_msg)
                .add_message(update_admin_msg))
        }
        Err(err) => Err(ContractError::Std(StdError::GenericErr {
            msg: err.to_string(),
        })),
    }
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps<ArchwayQuery>, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_binary(&CONFIG.load(deps.storage)?),
        QueryMsg::Share { recipient } => to_binary(&query_share(deps, recipient)?),
        QueryMsg::Shares { start_after, limit } => {
            to_binary(&query_shares(deps, start_after, limit)?)
        }
    }
}

fn query_share(deps: Deps<ArchwayQuery>, recipient: String) -> StdResult<Share> {
    let recipient = deps.api.addr_validate(&recipient)?;
    let share = SHARES.load(deps.storage, recipient)?;
    Ok(share)
}

fn query_shares(
    deps: Deps<ArchwayQuery>,
    start_after: Option<String>,
    limit: Option<u8>,
) -> StdResult<Vec<Share>> {
    let limit = limit.unwrap_or(10) as usize;
    let start = start_after.map(|s| {
        let recipient = deps.api.addr_validate(&s).unwrap();
        Bound::ExclusiveRaw(recipient.as_bytes().to_vec())
    });

    let shares = SHARES
        .range(deps.storage, start, None, Order::Ascending)
        .take(limit)
        .map(|item| {
            let (_, share) = item?;
            Ok(share)
        })
        .collect::<StdResult<Vec<Share>>>()?;

    Ok(shares)
}

// Used to validate that the total percentage does not exceed 100% and does not fall below 100%
fn check_share_percentages(shares: &Vec<Share>) -> Result<(), ContractError> {
    let total_percentage = shares
        .iter()
        .fold(Decimal::zero(), |acc, share| acc + share.percentage);

    if total_percentage > Decimal::one() {
        return Err(ContractError::PercentageLimitExceeded {});
    };
    if total_percentage < Decimal::one() {
        return Err(ContractError::PercentageLimitNotMet {});
    };

    Ok(())
}
