use anchor_lang::prelude::*;
use anchor_lang::solana_program::{clock, program_option::COption, sysvar};
use anchor_spl::token::{self, Mint, Token, TokenAccount};
use port_anchor_adaptor::{deposit_reserve, Deposit, redeem, Redeem};
use port_anchor_adaptor::port_accessor::{exchange_rate};

declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

#[program]
pub mod lossless_donation {
    use super::*;
    pub fn initialize(ctx: Context<Initialize>, nonce: u8) -> Result<()> {
        let donation_pool = &mut ctx.accounts.donation_pool;
        donation_pool.total_deposited = 0;
        donation_pool.total_donated = 0;
        // FTX Foundation
        donation_pool.donation_wallet = ctx.accounts.donation_address.key();
        donation_pool.donation_vault = ctx.accounts.donation_vault.key();
        donation_pool.token_mint = ctx.accounts.token_mint.key();
        donation_pool.token_vault = ctx.accounts.token_vault.key();
        donation_pool.yield_token_mint = ctx.accounts.yield_token_mint.key();
        donation_pool.yield_token_vault = ctx.accounts.yield_token_vault.key();
        donation_pool.user_stake_count = 0;
        donation_pool.nonce = nonce;

        Ok(())
    }

    pub fn create_user(ctx: Context<CreateUser>, nonce: u8) -> Result<()> {
        let user = &mut ctx.accounts.user;
        user.donation_pool = *ctx.accounts.donation_pool.to_account_info().key;
        user.owner = *ctx.accounts.owner.key;
        user.balance_staked = 0;
        user.nonce = nonce;

        let pool = &mut ctx.accounts.donation_pool;
        pool.user_stake_count = pool.user_stake_count.checked_add(1).unwrap();

        Ok(())
    }

    pub fn stake(ctx: Context<Stake>, amount: u64) -> Result<()> {
        if amount == 0 {
            return Err(ErrorCode::AmountMustBeGreaterThanZero.into());
        }

        let pool = &mut ctx.accounts.donation_pool;

        ctx.accounts.user.balance_staked = ctx
            .accounts
            .user
            .balance_staked
            .checked_add(amount)
            .unwrap();

        pool.total_deposited = pool.total_deposited.checked_add(amount as u128).unwrap();



        // Transfer tokens into the stake vault.
        {
            let cpi_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.stake_from_account.to_account_info(),
                    to: ctx.accounts.token_vault.to_account_info(),
                    authority: ctx.accounts.owner.to_account_info(), //todo use user account as signer
                },
            );
            token::transfer(cpi_ctx, amount)?;
        }

        // deposit into Port Finance
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.lending_program.clone(),
                Deposit {
                    source_liquidity: ctx.accounts.token_vault.to_account_info(),
                    destination_collateral: ctx.accounts.yield_token_vault.to_account_info(),
                    reserve: ctx.accounts.reserve.clone(),
                    reserve_liquidity_supply: ctx.accounts.reserve_liquidity_supply.clone(),
                    reserve_collateral_mint: ctx.accounts.token_mint.to_account_info(),
                    lending_market: ctx.accounts.lending_market.clone(),
                    lending_market_authority: ctx.accounts.lending_market_authority.clone(),
                    transfer_authority: ctx.accounts.transfer_authority.clone(),
                    clock: ctx.accounts.clock.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info()
                },
                pool_signer
            );
            deposit_reserve(cpi_ctx, ctx.accounts.token_vault.amount)?;
        }

        Ok(())
    }

    pub fn unstake(ctx: Context<Unstake>, amount: u64) -> Result<()> {
        if amount == 0 {
            return Err(ErrorCode::AmountMustBeGreaterThanZero.into());
        } if ctx.accounts.user.balance_staked < amount {
            return Err(ErrorCode::InsufficientFundUnstake.into());
        }

        let pool = &mut ctx.accounts.donation_pool;

        ctx.accounts.user.balance_staked = ctx
            .accounts
            .user
            .balance_staked
            .checked_sub(amount)
            .unwrap();

        pool.total_deposited = pool.total_deposited.checked_sub(amount as u128).unwrap(); 

        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Approve {
                    to: ctx.accounts.yield_token_vault.to_account_info(),
                    delegate: ctx.accounts.transfer_authority.to_account_info(),
                    authority: ctx.accounts.pool_signer.to_account_info(), //todo use user account as signer
                },
                pool_signer
            );
            token::approve(cpi_ctx, ctx.accounts.yield_token_vault.amount)?;
        }

        // withdraw from Port Finance to user's balance
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.lending_program.clone(),
                Redeem {
                    source_collateral: ctx.accounts.yield_token_vault.to_account_info(),
                    destination_liquidity: ctx.accounts.token_vault.to_account_info(),
                    reserve: ctx.accounts.reserve.clone(),
                    reserve_liquidity_supply: ctx.accounts.reserve_liquidity_supply.clone(),
                    reserve_collateral_mint: ctx.accounts.token_mint.to_account_info(),
                    lending_market: ctx.accounts.lending_market.clone(),
                    lending_market_authority: ctx.accounts.lending_market_authority.clone(),
                    transfer_authority: ctx.accounts.pool_signer.to_account_info(),
                    clock: ctx.accounts.clock.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info()
                },
                pool_signer
            );
            redeem(cpi_ctx, ctx.accounts.yield_token_vault.amount)?;
        }

        // Transfer tokens into the user's personal token vault.
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.token_vault.to_account_info(),
                    to: ctx.accounts.receiving_vault.to_account_info(),
                    authority: ctx.accounts.pool_signer.to_account_info(), //todo use user account as signer
                },
                pool_signer
            );
            token::transfer(cpi_ctx, amount)?;
        }

        // deposit into Port Finance
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.lending_program.clone(),
                Deposit {
                    source_liquidity: ctx.accounts.token_vault.to_account_info(),
                    destination_collateral: ctx.accounts.yield_token_vault.to_account_info(),
                    reserve: ctx.accounts.reserve.clone(),
                    reserve_liquidity_supply: ctx.accounts.reserve_liquidity_supply.clone(),
                    reserve_collateral_mint: ctx.accounts.token_mint.to_account_info(),
                    lending_market: ctx.accounts.lending_market.clone(),
                    lending_market_authority: ctx.accounts.lending_market_authority.clone(),
                    transfer_authority: ctx.accounts.transfer_authority.clone(),
                    clock: ctx.accounts.clock.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info()
                },
                pool_signer
            );
            deposit_reserve(cpi_ctx, ctx.accounts.token_vault.amount)?;
        }

        Ok(())
    }

    pub fn transfer_interest_to_charity(ctx: Context<TransferInterestToCharity>) -> Result<()> {
        let pool = &mut ctx.accounts.donation_pool;

        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Approve {
                    to: ctx.accounts.yield_token_vault.to_account_info(),
                    delegate: ctx.accounts.transfer_authority.to_account_info(),
                    authority: ctx.accounts.pool_signer.to_account_info(), //todo use user account as signer
                },
                pool_signer
            );
            token::approve(cpi_ctx, ctx.accounts.yield_token_vault.amount)?;
        }

        // withdraw from Port Finance to user's balance
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.lending_program.clone(),
                Redeem {
                    source_collateral: ctx.accounts.yield_token_vault.to_account_info(),
                    destination_liquidity: ctx.accounts.token_vault.to_account_info(),
                    reserve: ctx.accounts.reserve.clone(),
                    reserve_liquidity_supply: ctx.accounts.reserve_liquidity_supply.clone(),
                    reserve_collateral_mint: ctx.accounts.token_mint.to_account_info(),
                    lending_market: ctx.accounts.lending_market.clone(),
                    lending_market_authority: ctx.accounts.lending_market_authority.clone(),
                    transfer_authority: ctx.accounts.pool_signer.to_account_info(),
                    clock: ctx.accounts.clock.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info()
                },
                pool_signer
            );
            redeem(cpi_ctx, ctx.accounts.yield_token_vault.amount)?;
        }

        let interest = ctx.accounts.token_vault.amount - pool.total_deposited as u64;

        // Transfer tokens into the user's personal token vault.
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                token::Transfer {
                    from: ctx.accounts.token_vault.to_account_info(),
                    to: ctx.accounts.donation_vault.to_account_info(),
                    authority: ctx.accounts.pool_signer.to_account_info(), //todo use user account as signer
                },
                pool_signer
            );
            token::transfer(cpi_ctx, interest)?;
        }

        pool.total_donated += interest as u128;

        // deposit into Port Finance
        {
            let seeds = &[pool.to_account_info().key.as_ref(), &[pool.nonce]];
            let pool_signer = &[&seeds[..]];

            let cpi_ctx = CpiContext::new_with_signer(
                ctx.accounts.lending_program.clone(),
                Deposit {
                    source_liquidity: ctx.accounts.token_vault.to_account_info(),
                    destination_collateral: ctx.accounts.yield_token_vault.to_account_info(),
                    reserve: ctx.accounts.reserve.clone(),
                    reserve_liquidity_supply: ctx.accounts.reserve_liquidity_supply.clone(),
                    reserve_collateral_mint: ctx.accounts.token_mint.to_account_info(),
                    lending_market: ctx.accounts.lending_market.clone(),
                    lending_market_authority: ctx.accounts.lending_market_authority.clone(),
                    transfer_authority: ctx.accounts.transfer_authority.clone(),
                    clock: ctx.accounts.clock.to_account_info(),
                    token_program: ctx.accounts.token_program.to_account_info()
                },
                pool_signer
            );
            deposit_reserve(cpi_ctx, ctx.accounts.token_vault.amount)?;
        }

        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(nonce: u8)]
pub struct Initialize<'info> {
    #[account(zero)]
    pub donation_pool: Account<'info, DonationPool>,

    pub token_mint: Account<'info, Mint>,
    #[account(
        mut,
        constraint = token_vault.mint == token_mint.key(),
        constraint = token_vault.owner == signer.key(),
    )]
    pub token_vault: Account<'info, TokenAccount>,

    pub yield_token_mint: Account<'info, Mint>,
    #[account(
        mut,
        constraint = yield_token_vault.mint == yield_token_mint.key(),
        constraint = yield_token_vault.owner == signer.key(),
    )]
    pub yield_token_vault: Account<'info, TokenAccount>,

    pub donation_address: UncheckedAccount<'info>,
    #[account(
        mut,
        constraint = donation_vault.mint == token_mint.key(),
        constraint = donation_vault.owner == donation_address.key(),
    )]
    pub donation_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        seeds = [
            signer.to_account_info().key.as_ref()
        ],
        bump = nonce,
    )]
    pub signer: UncheckedAccount<'info>
}

#[derive(Accounts)]
#[instruction(nonce: u8)]
pub struct CreateUser<'info> {
    // Stake instance.
    #[account(
        mut,
    )]
    pub donation_pool: Box<Account<'info, DonationPool>>,
    // Member.
    #[account(
        init_if_needed,
        payer = owner,
        seeds = [
            owner.key.as_ref(),
            donation_pool.to_account_info().key.as_ref()
        ],
        bump,
    )]
    pub user: Box<Account<'info, User>>,

    #[account(mut)]
    pub owner: Signer<'info>,
    // Misc.
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Stake<'info> {
    #[account(
        mut,
        has_one = token_mint,
        has_one = token_vault,
        has_one = yield_token_vault
    )]
    pub donation_pool: Box<Account<'info, DonationPool>>,
    
    pub token_mint: Account<'info, Mint>,
    #[account(
        mut,
        constraint = token_vault.owner == *pool_signer.key,
    )]
    pub token_vault: Box<Account<'info, TokenAccount>>,

    // Port Finance Accounts
    #[account(
        mut,
        constraint = yield_token_vault.owner == *pool_signer.key,
    )]
    pub yield_token_vault: Box<Account<'info, TokenAccount>>,
    pub reserve: AccountInfo<'info>,
    pub reserve_liquidity_supply: AccountInfo<'info>,
    pub lending_market: AccountInfo<'info>,
    pub lending_market_authority: AccountInfo<'info>,
    pub transfer_authority: AccountInfo<'info>,

    // User.
    #[account(
        mut,
        has_one = owner,
        has_one = donation_pool,
        seeds = [
            owner.key.as_ref(),
            donation_pool.to_account_info().key.as_ref()
        ],
        bump = user.nonce,
    )]
    pub user: Box<Account<'info, User>>,
    pub owner: Signer<'info>,
    #[account(mut)]
    pub stake_from_account: Box<Account<'info, TokenAccount>>,

    // Program signers.
    #[account(
        seeds = [
            donation_pool.to_account_info().key.as_ref()
        ],
        bump = donation_pool.nonce,
    )]
    pub pool_signer: UncheckedAccount<'info>,

    // Misc.
    pub token_program: Program<'info, Token>,
    pub clock: Sysvar<'info, Clock>,
    pub lending_program: AccountInfo<'info>
}

#[derive(Accounts)]
pub struct Unstake<'info> {
    #[account(
        mut,
        has_one = token_mint,
        has_one = token_vault,
        has_one = yield_token_vault
    )]
    pub donation_pool: Box<Account<'info, DonationPool>>,

    pub token_mint: Account<'info, Mint>,
    #[account(
        mut,
        constraint = token_vault.owner == *pool_signer.key,
    )]
    pub token_vault: Box<Account<'info, TokenAccount>>,

    // Port Finance Accounts
    #[account(
        mut,
        constraint = yield_token_vault.owner == *pool_signer.key,
    )]
    pub yield_token_vault: Box<Account<'info, TokenAccount>>,
    pub reserve: AccountInfo<'info>,
    pub reserve_liquidity_supply: AccountInfo<'info>,
    pub lending_market: AccountInfo<'info>,
    pub lending_market_authority: AccountInfo<'info>,
    pub transfer_authority: AccountInfo<'info>,

    // User.
    #[account(
        mut,
        has_one = owner,
        has_one = donation_pool,
        seeds = [
            owner.key.as_ref(),
            donation_pool.to_account_info().key.as_ref()
        ],
        bump = user.nonce,
    )]
    pub user: Box<Account<'info, User>>,
    pub owner: Signer<'info>,

    #[account(mut)]
    pub receiving_vault: Box<Account<'info, TokenAccount>>,

    // Program signers.
    #[account(
        seeds = [
            donation_pool.to_account_info().key.as_ref()
        ],
        bump = donation_pool.nonce,
    )]
    pub pool_signer: UncheckedAccount<'info>,

    // Misc.
    pub token_program: Program<'info, Token>,
    pub clock: Sysvar<'info, Clock>,
    pub lending_program: AccountInfo<'info>
}

#[derive(Accounts)]
pub struct TransferInterestToCharity<'info> {
    #[account(
        mut,
        has_one = token_mint,
        has_one = token_vault,
        has_one = donation_vault,
        has_one = yield_token_vault
    )]
    pub donation_pool: Box<Account<'info, DonationPool>>,

    pub token_mint: Account<'info, Mint>,
    #[account(
        mut,
        constraint = token_vault.owner == *pool_signer.key,
    )]
    pub token_vault: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = donation_vault.owner == donation_pool.donation_wallet,
    )]
    pub donation_vault: Box<Account<'info, TokenAccount>>,

    // Port Finance Accounts
    #[account(
        mut,
        constraint = yield_token_vault.owner == *pool_signer.key,
    )]
    pub yield_token_vault: Box<Account<'info, TokenAccount>>,

    pub reserve: AccountInfo<'info>,
    pub reserve_liquidity_supply: AccountInfo<'info>,
    pub lending_market: AccountInfo<'info>,
    pub lending_market_authority: AccountInfo<'info>,
    pub transfer_authority: AccountInfo<'info>,

    pub owner: Signer<'info>,

    // Program signers.
    #[account(
        seeds = [
            donation_pool.to_account_info().key.as_ref()
        ],
        bump = donation_pool.nonce,
    )]
    pub pool_signer: UncheckedAccount<'info>,

    // Misc.
    pub token_program: Program<'info, Token>,
    pub clock: Sysvar<'info, Clock>,
    pub lending_program: AccountInfo<'info>
}

#[account]
pub struct DonationPool {
    /// The total amount of tokens in the pool.
    pub total_deposited: u128,
    /// The total amount of tokens donated to charity
    pub total_donated: u128,
    /// FTX Foundation Wallet
    pub donation_wallet: Pubkey,
    /// FTX Foundation Donation Vault
    pub donation_vault: Pubkey,
    /// Token Mint
    pub token_mint: Pubkey,
    /// Token Vault
    pub token_vault: Pubkey,
    /// Port Fi. Yield Token Mint
    pub yield_token_mint: Pubkey,
    /// Port Fi. Yield Token Vault
    pub yield_token_vault: Pubkey,
    /// User Count
    pub user_stake_count: u64,
    /// nonce
    pub nonce: u8
}

#[account]
#[derive(Default)]
pub struct User {
    /// Pool the this user belongs to.
    pub donation_pool: Pubkey,
    /// The owner of this account.
    pub owner: Pubkey,
    /// The amount staked.
    pub balance_staked: u64,
    /// Signer nonce.
    pub nonce: u8,
}

#[error]
pub enum ErrorCode {
    #[msg("Insufficient funds to unstake.")]
    InsufficientFundUnstake,
    #[msg("Amount must be greater than zero.")]
    AmountMustBeGreaterThanZero,
}
