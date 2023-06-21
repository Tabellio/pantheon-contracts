#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    instantiate2_address, to_binary, Binary, CodeInfoResponse, ContractInfoResponse, Decimal, Deps,
    DepsMut, Env, MessageInfo, Order, Response, StdResult, WasmMsg,
};
use cw2::set_contract_version;
use cw_storage_plus::Bound;

use archway_bindings::{ArchwayMsg, ArchwayQuery, ArchwayResult};

use crate::error::ContractError;
use crate::msg::{ExecuteMsg, InstantiateMsg, QueryMsg};
use crate::state::{Config, Share, CONFIG, SHARES};

use archway_reward_manager_utils::ExecuteMsg as ArchwayRewardManagerUtils;

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:archway-reward-manager";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

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

    // Setting the addresses for reward related processes
    // Sender is the one that can update the addresses
    // Sender is the one that will receive the rewards
    let metadata_msg = ArchwayMsg::UpdateContractMetadata {
        owner_address: Some(info.sender.to_string()),
        rewards_address: Some(info.sender.to_string()),
    };

    Ok(Response::new()
        .add_message(metadata_msg)
        .add_attribute("admin", info.sender))
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
        ExecuteMsg::AddCustomContact { code_id, msg } => {
            execute_add_custom_contract(deps, env, info, code_id, msg)
        }
        ExecuteMsg::LockContract {} => execute_lock_contract(deps, env, info),
        ExecuteMsg::DistributeRewards {} => unimplemented!(),
        ExecuteMsg::DistributeNativeTokens {} => unimplemented!(),
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

    let mut msgs: Vec<WasmMsg> = vec![];

    let creator = deps.api.addr_canonicalize(env.contract.address.as_str())?;
    let ContractInfoResponse {
        code_id: contract_code_id,
        ..
    } = deps
        .querier
        .query_wasm_contract_info(&env.contract.address)?;
    let CodeInfoResponse { checksum, .. } = deps.querier.query_wasm_code_info(contract_code_id)?;

    let salt = Binary::from(msg.clone());

    let address = deps
        .api
        .addr_humanize(&instantiate2_address(&checksum, &creator, &salt)?)?;

    msgs.push(WasmMsg::Instantiate2 {
        admin: Some(env.contract.address.to_string()),
        code_id,
        label: "".to_string(),
        msg,
        funds: info.funds,
        salt,
    });

    // Pull execute message from archway-reward-manager-utils package
    msgs.push(WasmMsg::Execute {
        contract_addr: address.to_string(),
        msg: to_binary(&ArchwayRewardManagerUtils::UpdateRewardMetadata {
            owner_address: Some(env.contract.address.to_string()),
            rewards_address: Some(env.contract.address.to_string()),
        })?,
        funds: vec![],
    });

    msgs.push(WasmMsg::UpdateAdmin {
        contract_addr: address.to_string(),
        admin: info.sender.to_string(),
    });

    Ok(Response::new().add_messages(msgs))
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

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps<ArchwayQuery>, _env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::Config {} => to_binary(&CONFIG.load(deps.storage)?),
        QueryMsg::Share { recipient } => unimplemented!(),
        QueryMsg::Shares { start_after, limit } => {
            to_binary(&query_shares(deps, start_after, limit)?)
        }
    }
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