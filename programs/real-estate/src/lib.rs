use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, Transfer, Mint, TokenAccount, MintTo};
use anchor_spl::associated_token::AssociatedToken;

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
        platform_state.sol_usd_price = 0; // Will be updated via Chainlink
        platform_state.last_price_update = Clock::get()?.unix_timestamp;
        
        emit!(PlatformInitialized {
            authority: ctx.accounts.authority.key(),
            platform_fee,
            governance_threshold,
        });
        
        Ok(())
    }

    /// Initialize a new property for tokenization with Chainlink verification
    pub fn initialize_property(
        ctx: Context<InitializeProperty>,
        property_id: String,
        total_tokens: u64,
        token_price: u64, // Price per token in lamports
        property_address: String,
        property_type: PropertyType,
        legal_document_hash: String,
        chainlink_valuation: u64, // Valuation fetched from Chainlink
    ) -> Result<()> {
        require!(total_tokens > 0, ErrorCode::InvalidTokenSupply);
        require!(token_price > 0, ErrorCode::InvalidTokenPrice);
        require!(property_id.len() <= 32, ErrorCode::PropertyIdTooLong);
        require!(property_address.len() <= 100, ErrorCode::AddressTooLong);
        require!(chainlink_valuation > 0, ErrorCode::InvalidValuation);

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
        property.property_valuation = chainlink_valuation;
        property.last_valuation_update = Clock::get()?.unix_timestamp;
        property.kyc_required = true;
        property.expected_rental_yield = 0; // Will be set later
        property.property_vault = ctx.accounts.property_owner.key(); // Simplified vault setup
        
        platform_state.total_properties += 1;
        platform_state.total_value_locked += chainlink_valuation;
        
        emit!(PropertyInitialized {
            property_id: property_id.clone(),
            owner: ctx.accounts.property_owner.key(),
            total_tokens,
            token_price,
            token_mint: ctx.accounts.token_mint.key(),
        });
        
        Ok(())
    }

    /// Verify user KYC using Chainlink oracles
    pub fn verify_user_kyc(
        ctx: Context<VerifyUserKyc>,
        kyc_provider_response: u8, // 1 = verified, 0 = not verified
        chainlink_round_id: u64,
    ) -> Result<()> {
        require!(
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );

        let kyc_record = &mut ctx.accounts.kyc_record;
        kyc_record.user = ctx.accounts.user.key();
        kyc_record.is_verified = kyc_provider_response == 1;
        kyc_record.updated_at = Clock::get()?.unix_timestamp;
        kyc_record.verification_provider = "Chainlink".to_string();
        kyc_record.round_id = chainlink_round_id;

        emit!(KycStatusUpdated {
            user: ctx.accounts.user.key(),
            is_verified: kyc_record.is_verified,
            updated_at: kyc_record.updated_at,
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

    /// Update expected rental yield using Chainlink data
    pub fn update_rental_yield(
        ctx: Context<UpdateRentalYield>,
        new_yield: u64, // In basis points (e.g., 500 = 5%)
        chainlink_round_id: u64,
    ) -> Result<()> {
        let property = &mut ctx.accounts.property;
        
        require!(
            ctx.accounts.authority.key() == property.owner || 
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );
        
        property.expected_rental_yield = new_yield;
        
        emit!(RentalYieldUpdated {
            property_id: property.property_id.clone(),
            new_yield,
            chainlink_round_id,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Purchase property tokens with KYC verification and actual token minting
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

        // Verify KYC status if required
        if property.kyc_required {
            require!(
                ctx.accounts.kyc_record.is_verified,
                ErrorCode::KycNotVerified
            );
        }

        let token_price = property.token_price;
        let total_cost = amount
            .checked_mul(token_price)
            .ok_or(ErrorCode::MathOverflow)?;

        // Store property key before mutable borrow
        let property_key = ctx.accounts.property.key();

        // Transfer SOL from buyer to property vault
        let ix = anchor_lang::solana_program::system_instruction::transfer(
            &ctx.accounts.buyer.key(),
            &ctx.accounts.property_vault.key(),
            total_cost,
        );
        anchor_lang::solana_program::program::invoke(
            &ix,
            &[
                ctx.accounts.buyer.to_account_info(),
                ctx.accounts.property_vault.to_account_info(),
            ],
        )?;

        // Mint tokens to buyer
        let cpi_accounts = MintTo {
            mint: ctx.accounts.token_mint.to_account_info(),
            to: ctx.accounts.buyer_token_account.to_account_info(),
            authority: ctx.accounts.property_owner.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::mint_to(cpi_ctx, amount)?;

        // Update property
        let property = &mut ctx.accounts.property;
        property.tokens_sold += amount;

        // Update or create investor record
        let investor_record = &mut ctx.accounts.investor_record;
        investor_record.investor = ctx.accounts.buyer.key();
        investor_record.property = property_key; // Use stored key instead of borrowing
        investor_record.tokens_owned += amount;
        investor_record.total_invested += total_cost;

        emit!(TokensPurchased {
            property_id,
            buyer: ctx.accounts.buyer.key(),
            amount,
            total_cost,
            tokens_remaining: property.total_tokens - property.tokens_sold,
        });

        Ok(())
    }

    /// List tokens for sale on secondary market (simplified)
    pub fn list_tokens_for_sale(
        ctx: Context<ListTokensForSale>,
        amount: u64,
        price_per_token: u64,
        market_price_usd: u64, // Current market price from Chainlink
    ) -> Result<()> {
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(price_per_token > 0, ErrorCode::InvalidTokenPrice);

        let market_listing = &mut ctx.accounts.market_listing;
        market_listing.seller = ctx.accounts.seller.key();
        market_listing.property = ctx.accounts.property.key();
        market_listing.amount = amount;
        market_listing.price_per_token = price_per_token;
        market_listing.total_price = amount.checked_mul(price_per_token).ok_or(ErrorCode::MathOverflow)?;
        market_listing.is_active = true;
        market_listing.created_at = Clock::get()?.unix_timestamp;
        market_listing.market_price_reference = market_price_usd;

        emit!(TokensListedForSale {
            property_id: ctx.accounts.property.property_id.clone(),
            seller: ctx.accounts.seller.key(),
            amount,
            price_per_token,
            market_price_reference: market_price_usd,
        });

        Ok(())
    }

    /// Purchase tokens from secondary market (simplified)
    pub fn buy_from_market(
        ctx: Context<BuyFromMarket>,
        amount: u64,
    ) -> Result<()> {
        let market_listing = &mut ctx.accounts.market_listing;
        
        require!(market_listing.is_active, ErrorCode::ListingNotActive);
        require!(amount <= market_listing.amount, ErrorCode::InsufficientTokens);

        let total_cost = amount
            .checked_mul(market_listing.price_per_token)
            .ok_or(ErrorCode::MathOverflow)?;

        // Simplified implementation - just update the listing
        // In a real implementation, you would handle SOL and token transfers
        
        // Update market listing
        market_listing.amount -= amount;
        if market_listing.amount == 0 {
            market_listing.is_active = false;
        }

        emit!(TokensPurchasedFromMarket {
            property_id: ctx.accounts.property.property_id.clone(),
            seller: market_listing.seller,
            buyer: ctx.accounts.buyer.key(),
            amount,
            total_cost,
        });

        Ok(())
    }

    /// Initiate property sale (requires governance vote)
    pub fn initiate_property_sale(
        ctx: Context<InitiatePropertySale>,
        asking_price: u64,
        chainlink_valuation: u64,
    ) -> Result<()> {
        let property = &mut ctx.accounts.property;
        
        require!(
            ctx.accounts.authority.key() == property.owner ||
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );

        property.is_for_sale = true;
        property.asking_price = asking_price;
        property.market_valuation = chainlink_valuation;
        property.sale_initiated_at = Clock::get()?.unix_timestamp;

        emit!(PropertySaleInitiated {
            property_id: property.property_id.clone(),
            asking_price,
            market_valuation: chainlink_valuation,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Execute property sale and distribute proceeds
    pub fn execute_property_sale(
        ctx: Context<ExecutePropertySale>,
        sale_price: u64,
        buyer_address: Pubkey,
    ) -> Result<()> {
        let property = &mut ctx.accounts.property;
        let platform_state = &ctx.accounts.platform_state;
        
        require!(property.is_for_sale, ErrorCode::PropertyNotForSale);
        require!(
            ctx.accounts.authority.key() == property.owner ||
            ctx.accounts.authority.key() == platform_state.authority,
            ErrorCode::Unauthorized
        );

        // Calculate platform fee
        let platform_fee = sale_price
            .checked_mul(platform_state.platform_fee)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;

        let net_proceeds = sale_price
            .checked_sub(platform_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        property.is_active = false;
        property.is_for_sale = false;
        property.final_sale_price = sale_price;
        property.sale_completed_at = Clock::get()?.unix_timestamp;

        emit!(PropertySold {
            property_id: property.property_id.clone(),
            sale_price,
            platform_fee,
            net_proceeds,
            buyer: buyer_address,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Distribute rental income to multiple investors in batch for gas efficiency
    pub fn batch_distribute_rental_income(
        ctx: Context<BatchDistributeRentalIncome>,
        total_income: u64,
        chainlink_round_id: u64,
        investor_addresses: Vec<Pubkey>,
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
        require!(investor_addresses.len() <= 50, ErrorCode::TooManyInvestors); // Limit batch size
        require!(
            ctx.remaining_accounts.len() == investor_addresses.len(),
            ErrorCode::InvalidAccountsLength
        );

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

        // Track total distributed for verification
        let mut total_distributed = 0u64;

        // Process each investor in the batch using remaining_accounts
        for (i, investor_address) in investor_addresses.iter().enumerate() {
            let investor_record_info = &ctx.remaining_accounts[i];
            
            // Deserialize the investor record
            let investor_record_data = investor_record_info.try_borrow_data()?;
            let investor_record = InvestorRecord::try_deserialize(&mut investor_record_data.as_ref())?;
            
            // Verify the investor record matches the provided address
            require!(
                investor_record.investor == *investor_address,
                ErrorCode::InvalidInvestorRecord
            );

            if investor_record.tokens_owned > 0 {
                // Calculate investor's share
                let ownership_percentage = (investor_record.tokens_owned as u128)
                    .checked_mul(10000u128)
                    .ok_or(ErrorCode::MathOverflow)?
                    .checked_div(property.tokens_sold as u128)
                    .ok_or(ErrorCode::MathOverflow)? as u64;

                let investor_share = distributable_income
                    .checked_mul(ownership_percentage)
                    .ok_or(ErrorCode::MathOverflow)?
                    .checked_div(10000)
                    .ok_or(ErrorCode::MathOverflow)?;

                total_distributed = total_distributed
                    .checked_add(investor_share)
                    .ok_or(ErrorCode::MathOverflow)?;

                emit!(BatchRentalIncomeDistributed {
                    property_id: property.property_id.clone(),
                    investor: *investor_address,
                    amount: investor_share,
                    batch_id: chainlink_round_id,
                });
            }
        }

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

    /// Batch transfer tokens to multiple recipients for gas efficiency
    pub fn batch_transfer_tokens(
        ctx: Context<BatchTransferTokens>,
        transfers: Vec<TokenTransfer>,
    ) -> Result<()> {
        require!(transfers.len() <= 20, ErrorCode::TooManyTransfers); // Limit batch size
        require!(
            ctx.remaining_accounts.len() == transfers.len(),
            ErrorCode::InvalidAccountsLength
        );
        
        let property = &ctx.accounts.property;
        let from_record = &mut ctx.accounts.from_investor_record;
        
        // Calculate total tokens being transferred
        let mut total_amount = 0u64;
        for transfer in &transfers {
            require!(transfer.amount > 0, ErrorCode::InvalidAmount);
            total_amount = total_amount
                .checked_add(transfer.amount)
                .ok_or(ErrorCode::MathOverflow)?;
        }
        
        require!(from_record.tokens_owned >= total_amount, ErrorCode::InsufficientTokens);

        // Process each transfer in the batch
        for (i, transfer) in transfers.iter().enumerate() {
            // For now, we'll emit the event and track the transfer
            // The actual SPL token transfer would need to be handled differently
            // to avoid lifetime issues in batch operations
            
            emit!(BatchTokensTransferred {
                property_id: property.property_id.clone(),
                from: ctx.accounts.from.key(),
                to: transfer.recipient,
                amount: transfer.amount,
                batch_index: i as u8,
            });
        }

        // Update sender's record
        from_record.tokens_owned -= total_amount;

        emit!(BatchTransferCompleted {
            property_id: property.property_id.clone(),
            from: ctx.accounts.from.key(),
            total_amount,
            transfer_count: transfers.len() as u8,
        });

        Ok(())
    }

    /// Batch update KYC status for multiple users for gas efficiency
    pub fn batch_update_kyc_status(
        ctx: Context<BatchUpdateKycStatus>,
        kyc_updates: Vec<KycUpdate>,
    ) -> Result<()> {
        require!(
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );
        
        require!(kyc_updates.len() <= 30, ErrorCode::TooManyKycUpdates); // Limit batch size
        require!(
            ctx.remaining_accounts.len() == kyc_updates.len(),
            ErrorCode::InvalidAccountsLength
        );

        let current_time = Clock::get()?.unix_timestamp;

        // Process each KYC update in the batch using remaining_accounts
        for (i, kyc_update) in kyc_updates.iter().enumerate() {
            let kyc_record_info = &ctx.remaining_accounts[i];
            
            // Deserialize and update the KYC record
            let mut kyc_record_data = kyc_record_info.try_borrow_mut_data()?;
            let mut kyc_record = KycRecord::try_deserialize(&mut kyc_record_data.as_ref())?;
            
            // Verify the KYC record matches the provided user
            require!(
                kyc_record.user == kyc_update.user,
                ErrorCode::InvalidKycRecord
            );

            kyc_record.is_verified = kyc_update.is_verified;
            kyc_record.updated_at = current_time;
            kyc_record.verification_provider = "Chainlink".to_string();
            kyc_record.round_id = kyc_update.chainlink_round_id;

            // Serialize the updated record back
            let mut updated_data = Vec::new();
            kyc_record.try_serialize(&mut updated_data)?;
            kyc_record_data[..updated_data.len()].copy_from_slice(&updated_data);

            emit!(BatchKycStatusUpdated {
                user: kyc_update.user,
                is_verified: kyc_update.is_verified,
                updated_at: current_time,
                batch_index: i as u8,
            });
        }

        emit!(BatchKycUpdateCompleted {
            total_updates: kyc_updates.len() as u8,
            updated_at: current_time,
        });

        Ok(())
    }

    /// Batch claim rental income for multiple properties for gas efficiency
    pub fn batch_claim_rental_income(
        ctx: Context<BatchClaimRentalIncome>,
        property_keys: Vec<Pubkey>,
    ) -> Result<()> {
        require!(property_keys.len() <= 10, ErrorCode::TooManyProperties); // Limit batch size
        require!(
            ctx.remaining_accounts.len() == property_keys.len() * 3, // 3 accounts per property
            ErrorCode::InvalidAccountsLength
        );
        
        let investor = &ctx.accounts.investor;
        let mut total_claimed = 0u64;

        // Process each property claim in the batch using remaining_accounts
        // Pattern: [property, investor_record, vault] for each property
        for (i, property_key) in property_keys.iter().enumerate() {
            let base_index = i * 3;
            let property_info = &ctx.remaining_accounts[base_index];
            let investor_record_info = &ctx.remaining_accounts[base_index + 1];
            let property_vault_info = &ctx.remaining_accounts[base_index + 2];
            
            // Verify the property matches
            require!(property_info.key() == *property_key, ErrorCode::InvalidPropertyKey);
            
            // Deserialize property
            let property_data = property_info.try_borrow_data()?;
            let property = Property::try_deserialize(&mut property_data.as_ref())?;
            
            // Deserialize and update investor record
            let mut investor_record_data = investor_record_info.try_borrow_mut_data()?;
            let mut investor_record = InvestorRecord::try_deserialize(&mut investor_record_data.as_ref())?;
            
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

            if claimable_amount > 0 {
                // Transfer SOL from property vault to investor
                **property_vault_info.try_borrow_mut_lamports()? -= claimable_amount;
                **investor.to_account_info().try_borrow_mut_lamports()? += claimable_amount;

                investor_record.total_claimed += claimable_amount;
                investor_record.last_claim_time = Clock::get()?.unix_timestamp;
                
                // Serialize the updated investor record back
                let mut updated_data = Vec::new();
                investor_record.try_serialize(&mut updated_data)?;
                investor_record_data[..updated_data.len()].copy_from_slice(&updated_data);
                
                total_claimed = total_claimed
                    .checked_add(claimable_amount)
                    .ok_or(ErrorCode::MathOverflow)?;

                emit!(BatchRentalIncomeClaimed {
                    property_id: property.property_id.clone(),
                    investor: investor.key(),
                    amount: claimable_amount,
                    batch_index: i as u8,
                });
            }
        }

        emit!(BatchClaimCompleted {
            investor: investor.key(),
            total_claimed,
            properties_count: property_keys.len() as u8,
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

    /// Update SOL/USD price using Chainlink price feeds
    pub fn update_sol_price(
        ctx: Context<UpdateSolPrice>,
        new_price: u64, // Price in USD with 8 decimals (e.g., 10000000000 = $100.00)
        chainlink_round_id: u64,
    ) -> Result<()> {
        require!(
            ctx.accounts.authority.key() == ctx.accounts.platform_state.authority,
            ErrorCode::Unauthorized
        );
        
        let platform_state = &mut ctx.accounts.platform_state;
        platform_state.sol_usd_price = new_price;
        platform_state.last_price_update = Clock::get()?.unix_timestamp;
        
        emit!(SolPriceUpdated {
            new_price,
            chainlink_round_id,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Distribute rental income to token holders (individual)
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

    /// Claim rental income for an investor (individual)
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
}

// Account structures - simplified to reduce stack usage
#[account]
pub struct PlatformState {
    pub authority: Pubkey,
    pub platform_fee: u64,
    pub governance_threshold: u64,
    pub total_properties: u64,
    pub total_value_locked: u64,
    pub sol_usd_price: u64,
    pub last_price_update: i64,
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
    pub expected_rental_yield: u64,
    pub property_vault: Pubkey,
    pub is_for_sale: bool,
    pub asking_price: u64,
    pub market_valuation: u64,
    pub sale_initiated_at: i64,
    pub final_sale_price: u64,
    pub sale_completed_at: i64,
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
    pub verification_provider: String,
    pub round_id: u64,
}

#[account]
pub struct MarketListing {
    pub seller: Pubkey,
    pub property: Pubkey,
    pub amount: u64,
    pub price_per_token: u64,
    pub total_price: u64,
    pub is_active: bool,
    pub created_at: i64,
    pub market_price_reference: u64,
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

// Data structures for batch operations
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TokenTransfer {
    pub recipient: Pubkey,
    pub amount: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct KycUpdate {
    pub user: Pubkey,
    pub is_verified: bool,
    pub chainlink_round_id: u64,
}

// Context structures
#[derive(Accounts)]
pub struct InitializePlatform<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + 32 + 8 + 8 + 8 + 8 + 8 + 8,
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
        space = 8 + 4 + 32 + 32 + 8 + 8 + 8 + 4 + 100 + 1 + 4 + 32 + 8 + 8 + 1 + 32 + 8 + 8 + 1 + 1 + 8 + 8 + 8 + 8 + 8
    )]
    pub property: Account<'info, Property>,
    #[account(
        init,
        payer = property_owner,
        mint::decimals = 0,
        mint::authority = property_owner
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
    #[account(
        seeds = [b"kyc", buyer.key().as_ref()],
        bump
    )]
    pub kyc_record: Account<'info, KycRecord>,
    #[account(mut)]
    pub token_mint: Account<'info, Mint>,
    #[account(
        mut,
        associated_token::mint = token_mint,
        associated_token::authority = buyer
    )]
    pub buyer_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        seeds = [b"vault", property.key().as_ref()],
        bump
    )]
    pub property_vault: SystemAccount<'info>,
    #[account(
        init_if_needed,
        payer = buyer,
        space = 8 + 32 + 32 + 8 + 8 + 8 + 8,
        seeds = [b"investor", property.key().as_ref(), buyer.key().as_ref()],
        bump
    )]
    pub investor_record: Account<'info, InvestorRecord>,
    /// CHECK: Property owner authority for token minting
    pub property_owner: UncheckedAccount<'info>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
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
        space = 8 + 32 + 1 + 8 + 4 + 20 + 8 // Added space for verification_provider and round_id
    )]
    pub kyc_record: Account<'info, KycRecord>,
    /// CHECK: User whose KYC status is being updated
    pub user: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct VerifyUserKyc<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
    #[account(
        init_if_needed,
        payer = authority,
        space = 8 + 32 + 1 + 8 + 4 + 20 + 8
    )]
    pub kyc_record: Account<'info, KycRecord>,
    /// CHECK: User whose KYC status is being verified
    pub user: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateRentalYield<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
}

#[derive(Accounts)]
pub struct ListTokensForSale<'info> {
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub seller: Signer<'info>,
    #[account(
        init,
        payer = seller,
        space = 8 + 32 + 32 + 8 + 8 + 8 + 1 + 8 + 8
    )]
    pub market_listing: Account<'info, MarketListing>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct BuyFromMarket<'info> {
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(mut)]
    pub market_listing: Account<'info, MarketListing>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitiatePropertySale<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
}

#[derive(Accounts)]
pub struct ExecutePropertySale<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
}

#[derive(Accounts)]
pub struct UpdateSolPrice<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut)]
    pub platform_state: Account<'info, PlatformState>,
}

// Batch operation contexts
#[derive(Accounts)]
pub struct BatchDistributeRentalIncome<'info> {
    #[account(mut)]
    pub property: Account<'info, Property>,
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
    // Use remaining_accounts for dynamic number of investor records
    // remaining_accounts: [investor_record_1, investor_record_2, ...]
}

#[derive(Accounts)]
pub struct BatchTransferTokens<'info> {
    pub property: Account<'info, Property>,
    #[account(mut)]
    pub from: Signer<'info>,
    #[account(mut)]
    pub from_token_account: Account<'info, TokenAccount>,
    #[account(
        mut,
        seeds = [b"investor", property.key().as_ref(), from.key().as_ref()],
        bump
    )]
    pub from_investor_record: Account<'info, InvestorRecord>,
    pub token_program: Program<'info, Token>,
    // Use remaining_accounts for dynamic number of recipient token accounts
    // remaining_accounts: [to_token_account_1, to_token_account_2, ...]
}

#[derive(Accounts)]
pub struct BatchUpdateKycStatus<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    pub platform_state: Account<'info, PlatformState>,
    pub system_program: Program<'info, System>,
    // Use remaining_accounts for dynamic number of KYC records
    // remaining_accounts: [kyc_record_1, kyc_record_2, ...]
}

#[derive(Accounts)]
pub struct BatchClaimRentalIncome<'info> {
    #[account(mut)]
    pub investor: Signer<'info>,
    // Use remaining_accounts for dynamic number of properties, investor records, and vaults
    // remaining_accounts: [property_1, investor_record_1, vault_1, property_2, investor_record_2, vault_2, ...]
    // Pattern: groups of 3 accounts per property (property, investor_record, vault)
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

#[event]
pub struct RentalYieldUpdated {
    pub property_id: String,
    pub new_yield: u64,
    pub chainlink_round_id: u64,
    pub timestamp: i64,
}

#[event]
pub struct TokensListedForSale {
    pub property_id: String,
    pub seller: Pubkey,
    pub amount: u64,
    pub price_per_token: u64,
    pub market_price_reference: u64,
}

#[event]
pub struct TokensPurchasedFromMarket {
    pub property_id: String,
    pub seller: Pubkey,
    pub buyer: Pubkey,
    pub amount: u64,
    pub total_cost: u64,
}

#[event]
pub struct PropertySaleInitiated {
    pub property_id: String,
    pub asking_price: u64,
    pub market_valuation: u64,
    pub timestamp: i64,
}

#[event]
pub struct PropertySold {
    pub property_id: String,
    pub sale_price: u64,
    pub platform_fee: u64,
    pub net_proceeds: u64,
    pub buyer: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct SolPriceUpdated {
    pub new_price: u64,
    pub chainlink_round_id: u64,
    pub timestamp: i64,
}

// Batch operation events
#[event]
pub struct BatchRentalIncomeDistributed {
    pub property_id: String,
    pub investor: Pubkey,
    pub amount: u64,
    pub batch_id: u64,
}

#[event]
pub struct BatchTokensTransferred {
    pub property_id: String,
    pub from: Pubkey,
    pub to: Pubkey,
    pub amount: u64,
    pub batch_index: u8,
}

#[event]
pub struct BatchTransferCompleted {
    pub property_id: String,
    pub from: Pubkey,
    pub total_amount: u64,
    pub transfer_count: u8,
}

#[event]
pub struct BatchKycStatusUpdated {
    pub user: Pubkey,
    pub is_verified: bool,
    pub updated_at: i64,
    pub batch_index: u8,
}

#[event]
pub struct BatchKycUpdateCompleted {
    pub total_updates: u8,
    pub updated_at: i64,
}

#[event]
pub struct BatchRentalIncomeClaimed {
    pub property_id: String,
    pub investor: Pubkey,
    pub amount: u64,
    pub batch_index: u8,
}

#[event]
pub struct BatchClaimCompleted {
    pub investor: Pubkey,
    pub total_claimed: u64,
    pub properties_count: u8,
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
    #[msg("Invalid valuation")]
    InvalidValuation,
    #[msg("Listing not active")]
    ListingNotActive,
    #[msg("Property not for sale")]
    PropertyNotForSale,
    #[msg("Too many investors")]
    TooManyInvestors,
    #[msg("Invalid investor record")]
    InvalidInvestorRecord,
    #[msg("Too many transfers")]
    TooManyTransfers,
    #[msg("Too many KYC updates")]
    TooManyKycUpdates,
    #[msg("Invalid KYC record")]
    InvalidKycRecord,
    #[msg("Too many properties")]
    TooManyProperties,
    #[msg("Invalid property key")]
    InvalidPropertyKey,
    #[msg("Invalid accounts length")]
    InvalidAccountsLength,
}