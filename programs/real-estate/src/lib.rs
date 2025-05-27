use anchor_lang::prelude::*;

declare_id!("7BwJmWypzV9WokmhxHZEjisoiBmpNhzcCnr8wQX3Kn9w");

#[program]
pub mod real_estate {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
