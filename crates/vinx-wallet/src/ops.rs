//! Transactions utilisateur : construction, signature et envoi.

use crate::client::Client;
use anyhow::{Result, bail};
use vinx_core::{Amount, Transaction, TxBody};
use vinx_crypto::{Address, KeyPair, PublicKey};

/// Signe `body` avec le prochain nonce du compte et l'envoie au nœud.
///
/// Le compte émetteur est `from`, ou à défaut l'adresse dérivée de la clé.
/// Après une rotation de clé, il faut indiquer `from` : l'adresse reste celle
/// de la clé d'origine. `fee` vaut par défaut les frais conseillés par le nœud
/// (le minimum du réseau, sauf en cas d'encombrement).
pub fn submit(
    client: &Client,
    keypair: &KeyPair,
    from: Option<Address>,
    body: TxBody,
    fee: Option<Amount>,
) -> Result<String> {
    let status = client.status()?;
    let sender = from.unwrap_or_else(|| Address::from_public_key(&keypair.public_key()));
    let Some(account) = client.account(&sender)? else {
        bail!("le compte {sender} n'existe pas encore : il doit d'abord recevoir des VINX");
    };
    let nonce = account
        .nonce
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("nonce épuisé"))?;
    let tx = Transaction {
        chain_id: status.chain_id,
        sender,
        nonce,
        fee: match fee {
            Some(f) => f,
            None => client.recommended_fee()?,
        },
        body,
    };
    // La clé publique n'est jointe qu'à la première dépense du compte.
    client.submit(&tx.sign(keypair, !account.has_key))
}

pub fn transfer(
    client: &Client,
    keypair: &KeyPair,
    from: Option<Address>,
    to: Address,
    amount: Amount,
    memo: Option<String>,
    fee: Option<Amount>,
) -> Result<String> {
    let memo = memo.map(String::into_bytes);
    submit(
        client,
        keypair,
        from,
        TxBody::Transfer { to, amount, memo },
        fee,
    )
}

pub fn set_memo_required(
    client: &Client,
    keypair: &KeyPair,
    from: Option<Address>,
    required: bool,
) -> Result<String> {
    submit(
        client,
        keypair,
        from,
        TxBody::SetMemoRequired { required },
        None,
    )
}

pub fn rotate_key(
    client: &Client,
    keypair: &KeyPair,
    from: Option<Address>,
    new_key: PublicKey,
) -> Result<String> {
    submit(client, keypair, from, TxBody::RotateKey { new_key }, None)
}
