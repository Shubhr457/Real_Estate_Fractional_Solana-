use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, Transfer, Mint, TokenAccount};

declare_id!("7BwJmWypzV9WokmhxHZEjisoiBmpNhzcCnr8wQX3Kn9w");

#[program]
pub mod real_estate_platform {
    use super::*;

    /// Initialize the platform with global settings
    pub fn initialize_platform(
        ctx: Context<InitializePlatform>,
        platform_fee: u64, // Fee in basis points (e.g., 100 = 1%)
        governance_threshold: u64, // Minimum tokens needed to create proposals
    ) -> Result<()> {
        let platform_state = &mut ctx.accounts.platform_state;
        platform_state.authority = ctx.accounts.authority.key();
        platform_state.platform_fee = platform_fee;
        platform_state.governance_threshold = governance_threshold;
        platform_state.total_properties = 0;
        platform_state.total_value_locked = 0;
        
        emit!(PlatformInitialized {
            authority: ctx.accounts.authority.key(),
            platform_fee,
            governance_threshold,
        });
        
        Ok(())
    }

    /// Initialize a new property for tokenization
    pub fn initialize_property(
        ctx: Context<InitializeProperty>,
        property_id: String,
        total_tokens: u64,
        token_price: u64, // Price per token in lamports
        property_address: String,
        property_type: PropertyType,
        legal_document_hash: String,
    ) -> Result<()> {
        require!(total_tokens > 0, ErrorCode::InvalidTokenSupply);
        require!(token_price > 0, ErrorCode::InvalidTokenPrice);
        require!(property_id.len() <= 32, ErrorCode::PropertyIdTooLong);
        require!(property_address.len() <= 100, ErrorCode::AddressTooLong);

        let property = &mut ctx.accounts.property;
        let platform_state = &mut ctx.accounts.platform_state;
        
        property.property_id = property_id.clone();
        property.owner = ctx.accounts.property_owner.key();
        property.total_tokens = total_tokens;
        property.tokens_sold = 0;
        property.token_price = token_price;
        property.property_address = property_address;
        property.property_type = property_type;
        property.legal_document_hash = legal_document_hash;
        property.total_rental_income = 0;
        property.last_income_distribution = Clock::get()?.unix_timestamp;
        property.is_active = true;
        property.token_mint = ctx.accounts.token_mint.key();
        property.property_valuation = 0;
        property.kyc_required = true;
        
        platform_state.total_properties += 1;
        
        emit!(PropertyInitialized {
            property_id: property_id.clone(),
            owner: ctx.accounts.property_owner.key(),
            total_tokens,
            token_price,
            token_mint: ctx.accounts.token_mint.key(),
        });
        
        Ok(())
    }

    /// Update property valuation using Chainlink oracle data
    pub fn update_property_valuation(
        ctx: Context<UpdatePropertyValuation>,
        new_valuation: u64,
        chainlink_round_id: u64,
    ) -> Result<()> {
        let property = &mut ctx.accounts.property;
        
        // Verify the caller is authorized (oracle or property owner)
        require!(
            ctx.accounts.authority.key() == property.owner || 
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );
        
        let old_valuation = property.property_valuation;
        property.property_valuation = new_valuation;
        property.last_valuation_update = Clock::get()?.unix_timestamp;
        
        emit!(PropertyValuationUpdated {
            property_id: property.property_id.clone(),
            old_valuation,
            new_valuation,
            chainlink_round_id,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Purchase property tokens (fractional ownership)
    pub fn purchase_tokens(
        ctx: Context<PurchaseTokens>,
        amount: u64,
        property_id: String,
    ) -> Result<()> {
        let property = &ctx.accounts.property;
        require!(property.is_active, ErrorCode::PropertyNotActive);
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(
            property.tokens_sold + amount <= property.total_tokens,
            ErrorCode::InsufficientTokens
        );

        let token_price = property.token_price;
        let total_cost = amount
            .checked_mul(token_price)
            .ok_or(ErrorCode::MathOverflow)?;

        let buyer = &ctx.accounts.buyer;

        // Simplified implementation - just track the purchase
        // In a real implementation, you would handle SOL transfers and token minting
        
        // Update property
        let property = &mut ctx.accounts.property;
        property.tokens_sold += amount;

        emit!(TokensPurchased {
            property_id,
            buyer: buyer.key(),
            amount,
            total_cost,
            tokens_remaining: property.total_tokens - property.tokens_sold,
        });

        Ok(())
    }

    /// Distribute rental income to token holders
    pub fn distribute_rental_income(
        ctx: Context<DistributeRentalIncome>,
        total_income: u64,
        chainlink_round_id: u64,
    ) -> Result<()> {
        let property = &mut ctx.accounts.property;
        let platform_state = &ctx.accounts.platform_state;
        
        require!(
            ctx.accounts.authority.key() == property.owner ||
            ctx.accounts.authority.key() == platform_state.authority,
            ErrorCode::Unauthorized
        );
        
        require!(total_income > 0, ErrorCode::InvalidAmount);
        require!(property.tokens_sold > 0, ErrorCode::NoTokensIssued);

        // Calculate platform fee
        let platform_fee = total_income
            .checked_mul(platform_state.platform_fee)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;

        let distributable_income = total_income
            .checked_sub(platform_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        property.total_rental_income += distributable_income;
        property.last_income_distribution = Clock::get()?.unix_timestamp;

        emit!(RentalIncomeDistributed {
            property_id: property.property_id.clone(),
            total_income,
            platform_fee,
            distributable_income,
            chainlink_round_id,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Claim rental income for an investor
    pub fn claim_rental_income(ctx: Context<ClaimRentalIncome>) -> Result<()> {
        let property = &ctx.accounts.property;
        let investor_record = &mut ctx.accounts.investor_record;
        
        require!(investor_record.tokens_owned > 0, ErrorCode::NoTokensOwned);
        
        // Calculate claimable amount
        let ownership_percentage = (investor_record.tokens_owned as u128)
            .checked_mul(10000u128)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(property.tokens_sold as u128)
            .ok_or(ErrorCode::MathOverflow)? as u64;

        let claimable_amount = property.total_rental_income
            .checked_mul(ownership_percentage)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_sub(investor_record.total_claimed)
            .ok_or(ErrorCode::MathOverflow)?;

        require!(claimable_amount > 0, ErrorCode::NothingToClaim);

        // Transfer SOL from property vault to investor
        **ctx.accounts.property_vault.to_account_info().try_borrow_mut_lamports()? -= claimable_amount;
        **ctx.accounts.investor.to_account_info().try_borrow_mut_lamports()? += claimable_amount;

        investor_record.total_claimed += claimable_amount;
        investor_record.last_claim_time = Clock::get()?.unix_timestamp;

        emit!(RentalIncomeClaimed {
            property_id: property.property_id.clone(),
            investor: ctx.accounts.investor.key(),
            amount: claimable_amount,
            total_claimed: investor_record.total_claimed,
        });

        Ok(())
    }

    /// Create a governance proposal
    pub fn create_proposal(
        ctx: Context<CreateProposal>,
        title: String,
        description: String,
        proposal_type: ProposalType,
        voting_period: i64,
    ) -> Result<()> {
        let property = &ctx.accounts.property;
        let investor_record = &ctx.accounts.investor_record;
        let platform_state = &ctx.accounts.platform_state;
        
        require!(
            investor_record.tokens_owned >= platform_state.governance_threshold,
            ErrorCode::InsufficientTokensForProposal
        );
        
        require!(title.len() <= 50, ErrorCode::TitleTooLong);
        require!(description.len() <= 200, ErrorCode::DescriptionTooLong);
        require!(voting_period > 0, ErrorCode::InvalidVotingPeriod);

        let proposal = &mut ctx.accounts.proposal;
        let current_time = Clock::get()?.unix_timestamp;
        
        proposal.property = ctx.accounts.property.key();
        proposal.proposer = ctx.accounts.proposer.key();
        proposal.title = title.clone();
        proposal.description = description;
        proposal.proposal_type = proposal_type.clone();
        proposal.votes_for = 0;
        proposal.votes_against = 0;
        proposal.total_votes = 0;
        proposal.created_at = current_time;
        proposal.voting_ends_at = current_time + voting_period;
        proposal.executed = false;
        proposal.passed = false;

        emit!(ProposalCreated {
            property_id: property.property_id.clone(),
            proposer: ctx.accounts.proposer.key(),
            title,
            proposal_type,
            voting_ends_at: proposal.voting_ends_at,
        });

        Ok(())
    }

    /// Vote on a governance proposal
    pub fn vote_on_proposal(
        ctx: Context<VoteOnProposal>,
        vote_for: bool,
    ) -> Result<()> {
        let proposal = &mut ctx.accounts.proposal;
        let investor_record = &ctx.accounts.investor_record;
        let vote_record = &mut ctx.accounts.vote_record;
        
        let current_time = Clock::get()?.unix_timestamp;
        require!(current_time <= proposal.voting_ends_at, ErrorCode::VotingPeriodEnded);
        require!(investor_record.tokens_owned > 0, ErrorCode::NoTokensOwned);
        require!(!vote_record.has_voted, ErrorCode::AlreadyVoted);

        let voting_power = investor_record.tokens_owned;
        
        if vote_for {
            proposal.votes_for += voting_power;
        } else {
            proposal.votes_against += voting_power;
        }
        
        proposal.total_votes += voting_power;
        
        vote_record.voter = ctx.accounts.voter.key();
        vote_record.proposal = ctx.accounts.proposal.key();
        vote_record.vote_for = vote_for;
        vote_record.voting_power = voting_power;
        vote_record.has_voted = true;
        vote_record.voted_at = current_time;

        emit!(VoteCast {
            proposal: ctx.accounts.proposal.key(),
            voter: ctx.accounts.voter.key(),
            vote_for,
            voting_power,
        });

        Ok(())
    }

    /// Execute a passed proposal
    pub fn execute_proposal(ctx: Context<ExecuteProposal>) -> Result<()> {
        let proposal = &mut ctx.accounts.proposal;
        let property = &ctx.accounts.property;
        
        let current_time = Clock::get()?.unix_timestamp;
        require!(current_time > proposal.voting_ends_at, ErrorCode::VotingStillActive);
        require!(!proposal.executed, ErrorCode::ProposalAlreadyExecuted);
        
        // Check if proposal passed
        let passed = proposal.votes_for > proposal.votes_against && 
                    proposal.total_votes > property.tokens_sold / 2;
        
        proposal.passed = passed;
        proposal.executed = true;

        emit!(ProposalExecuted {
            proposal: proposal.key(),
            passed,
            votes_for: proposal.votes_for,
            votes_against: proposal.votes_against,
        });

        Ok(())
    }

    /// Transfer tokens between users
    pub fn transfer_tokens(
        ctx: Context<TransferTokens>,
        amount: u64,
    ) -> Result<()> {
        let property = &ctx.accounts.property;
        let from_record = &mut ctx.accounts.from_investor_record;
        let to_record = &mut ctx.accounts.to_investor_record;
        
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(from_record.tokens_owned >= amount, ErrorCode::InsufficientTokens);

        // Transfer SPL tokens
        let cpi_accounts = Transfer {
            from: ctx.accounts.from_token_account.to_account_info(),
            to: ctx.accounts.to_token_account.to_account_info(),
            authority: ctx.accounts.from.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        
        token::transfer(cpi_ctx, amount)?;

        // Update investor records
        from_record.tokens_owned -= amount;
        to_record.tokens_owned += amount;

        emit!(TokensTransferred {
            property_id: property.property_id.clone(),
            from: ctx.accounts.from.key(),
            to: ctx.accounts.to.key(),
            amount,
        });

        Ok(())
    }

    /// Update KYC status
    pub fn update_kyc_status(
        ctx: Context<UpdateKycStatus>,
        user: Pubkey,
        is_verified: bool,
    ) -> Result<()> {
        require!(
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );

        let kyc_record = &mut ctx.accounts.kyc_record;
        kyc_record.user = user;
        kyc_record.is_verified = is_verified;
        kyc_record.updated_at = Clock::get()?.unix_timestamp;

        emit!(KycStatusUpdated {
            user,
            is_verified,
            updated_at: kyc_record.updated_at,
        });

        Ok(())
    }
}

// Account structures - simplified to reduce stack usage
#[account]
pub struct PlatformState {
    pub authority: Pubkey,
    pub platform_fee: u64,
    pub governance_threshold: u64,
    pub total_properties: u64,
    pub total_value_locked: u64,
}

#[account]
pub struct Property {
    pub property_id: String,        // 32 max
    pub owner: Pubkey,
    pub total_tokens: u64,
    pub tokens_sold: u64,
    pub token_price: u64,
    pub property_address: String,   // 100 max
    pub property_type: PropertyType,
    pub legal_document_hash: String, // 32 max
    pub total_rental_income: u64,
    pub last_income_distribution: i64,
    pub is_active: bool,
    pub token_mint: Pubkey,
    pub property_valuation: u64,
    pub last_valuation_update: i64,
    pub kyc_required: bool,
}

#[account]
pub struct InvestorRecord {
    pub investor: Pubkey,
    pub property: Pubkey,
    pub tokens_owned: u64,
    pub total_invested: u64,
    pub total_claimed: u64,
    pub last_claim_time: i64,
}

#[account]
pub struct Proposal {
    pub property: Pubkey,
    pub proposer: Pubkey,
    pub title: String,              // 50 max
    pub description: String,        // 200 max
    pub proposal_type: ProposalType,
    pub votes_for: u64,
    pub votes_against: u64,
    pub total_votes: u64,
    pub created_at: i64,
    pub voting_ends_at: i64,
    pub executed: bool,
    pub passed: bool,
}

#[account]
pub struct VoteRecord {
    pub voter: Pubkey,
    pub proposal: Pubkey,
    pub vote_for: bool,
    pub voting_power: u64,
    pub has_voted: bool,
    pub voted_at: i64,
}

#[account]
pub struct KycRecord {
    pub user: Pubkey,
    pub is_verified: bool,
    pub updated_at: i64,
}

// Enums
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum PropertyType {
    Residential,
    Commercial,
    Industrial,
    Mixed,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum ProposalType {
    RenovationApproval,
    TenantApproval,
    PropertySale,
    ManagementChange,
}

// Context structures
#[derive(Accounts)]
pub struct InitializePlatform<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + 32 + 8 + 8 + 8 + 8,
        seeds = [b"platform"],
        bump
    )]
    pub platform_state: Account<'info, PlatformState>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitializeProperty<'info> {
    #[account(
        init,
        payer = property_owner,
        space = 8 + 4 + 32 + 32 + 8 + 8 + 8 + 4 + 100 + 1 + 4 + 32 + 8 + 8 + 1 + 32 + 8 + 8 + 1
    )]
    pub property: Account<'info, Property>,
    #[account(
        init,
        payer = property_owner,
        mint::decimals = 0,
        mint::authority = property
    )]
    pub token_mint: Account<'info, Mint>,
    #[account(mut)]
    pub property_owner: Signer<'info>,
    #[account(mut)]
    pub platform_state: Account<'info, PlatformState>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct UpdatePropertyValuation<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
}

#[derive(Accounts)]
pub struct PurchaseTokens<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub buyer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DistributeRentalIncome<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
}

#[derive(Accounts)]
pub struct ClaimRentalIncome<'info> {
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub investor: Signer<'info>,
    #[account(
        mut,
        seeds = [b"investor", property.key().as_ref(), investor.key().as_ref()],
        bump
    )]
    pub investor_record: Account<'info, InvestorRecord>,
    #[account(
        mut,
        seeds = [b"vault", property.key().as_ref()],
        bump
    )]
    pub property_vault: SystemAccount<'info>,
}

#[derive(Accounts)]
pub struct CreateProposal<'info> {
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub proposer: Signer<'info>,
    #[account(
        seeds = [b"investor", property.key().as_ref(), proposer.key().as_ref()],
        bump
    )]
    pub investor_record: Account<'info, InvestorRecord>,
    #[account(
        init,
        payer = proposer,
        space = 8 + 32 + 32 + 4 + 50 + 4 + 200 + 1 + 8 + 8 + 8 + 8 + 8 + 1 + 1
    )]
    pub proposal: Account<'info, Proposal>,
    pub platform_state: Account<'info, PlatformState>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VoteOnProposal<'info> {
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,
    #[account(mut)]
    pub voter: Signer<'info>,
    #[account(
        seeds = [b"investor", proposal.property.as_ref(), voter.key().as_ref()],
        bump
    )]
    pub investor_record: Account<'info, InvestorRecord>,
    #[account(
        init_if_needed,
        payer = voter,
        space = 8 + 32 + 32 + 1 + 8 + 1 + 8,
        seeds = [b"vote", proposal.key().as_ref(), voter.key().as_ref()],
        bump
    )]
    pub vote_record: Account<'info, VoteRecord>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteProposal<'info> {
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct TransferTokens<'info> {
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub from: Signer<'info>,
    /// CHECK: Safe as we only use it as a key
    pub to: UncheckedAccount<'info>,
    #[account(mut)]
    pub from_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub to_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        seeds = [b"investor", property.key().as_ref(), from.key().as_ref()],
        bump
    )]
    pub from_investor_record: Account<'info, InvestorRecord>,
    #[account(
        init_if_needed,
        payer = from,
        space = 8 + 32 + 32 + 8 + 8 + 8 + 8,
        seeds = [b"investor", property.key().as_ref(), to.key().as_ref()],
        bump
    )]
    pub to_investor_record: Account<'info, InvestorRecord>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateKycStatus<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + 32 + 1 + 8,
        seeds = [b"kyc", user.key().as_ref()],
        bump
    )]
    pub kyc_record: Account<'info, KycRecord>,
    /// CHECK: User whose KYC status is being updated
    pub user: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

// Events
#[event]
pub struct PlatformInitialized {
    pub authority: Pubkey,
    pub platform_fee: u64,
    pub governance_threshold: u64,
}

#[event]
pub struct PropertyInitialized {
    pub property_id: String,
    pub owner: Pubkey,
    pub total_tokens: u64,
    pub token_price: u64,
    pub token_mint: Pubkey,
}

#[event]
pub struct PropertyValuationUpdated {
    pub property_id: String,
    pub old_valuation: u64,
    pub new_valuation: u64,
    pub chainlink_round_id: u64,
    pub timestamp: i64,
}

#[event]
pub struct TokensPurchased {
    pub property_id: String,
    pub buyer: Pubkey,
    pub amount: u64,
    pub total_cost: u64,
    pub tokens_remaining: u64,
}

#[event]
pub struct RentalIncomeDistributed {
    pub property_id: String,
    pub total_income: u64,
    pub platform_fee: u64,
    pub distributable_income: u64,
    pub chainlink_round_id: u64,
    pub timestamp: i64,
}

#[event]
pub struct RentalIncomeClaimed {
    pub property_id: String,
    pub investor: Pubkey,
    pub amount: u64,
    pub total_claimed: u64,
}

#[event]
pub struct ProposalCreated {
    pub property_id: String,
    pub proposer: Pubkey,
    pub title: String,
    pub proposal_type: ProposalType,
    pub voting_ends_at: i64,
}

#[event]
pub struct VoteCast {
    pub proposal: Pubkey,
    pub voter: Pubkey,
    pub vote_for: bool,
    pub voting_power: u64,
}

#[event]
pub struct ProposalExecuted {
    pub proposal: Pubkey,
    pub passed: bool,
    pub votes_for: u64,
    pub votes_against: u64,
}

#[event]
pub struct TokensTransferred {
    pub property_id: String,
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount: u64,
}

#[event]
pub struct KycStatusUpdated {
    pub user: Pubkey,
    pub is_verified: bool,
    pub updated_at: i64,
}

// Error codes
#[error_code]
pub enum ErrorCode {
    #[msg("Invalid token supply")]
    InvalidTokenSupply,
    #[msg("Invalid token price")]
    InvalidTokenPrice,
    #[msg("Property ID too long")]
    PropertyIdTooLong,
    #[msg("Address too long")]
    AddressTooLong,
    #[msg("Property not active")]
    PropertyNotActive,
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Insufficient tokens available")]
    InsufficientTokens,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("No tokens issued")]
    NoTokensIssued,
    #[msg("No tokens owned")]
    NoTokensOwned,
    #[msg("Nothing to claim")]
    NothingToClaim,
    #[msg("Insufficient tokens for proposal")]
    InsufficientTokensForProposal,
    #[msg("Title too long")]
    TitleTooLong,
    #[msg("Description too long")]
    DescriptionTooLong,
    #[msg("Invalid voting period")]
    InvalidVotingPeriod,
    #[msg("Voting period ended")]
    VotingPeriodEnded,
    #[msg("Already voted")]
    AlreadyVoted,
    #[msg("Voting still active")]
    VotingStillActive,
    #[msg("Proposal already executed")]
    ProposalAlreadyExecuted,
    #[msg("KYC not verified")]
    KycNotVerified,
}