#[cfg(test)] mod test;

pub mod r#type;
use r#type::*;

// std
// std
use std::{str::FromStr, time::Duration};
// crates.io
use bitcoin::{Address, Network};
use reqwest::{Client as ReqwestClient, ClientBuilder as ReqwestClientBuilder};
use serde::{de::DeserializeOwned, Serialize};
use tokio::time;
// atomicalsir
use crate::{prelude::*, util};

pub trait Config {
	fn network(&self) -> &Network;
	fn base_uris(&self) -> &[String];
}

pub trait Http {
	async fn post<U, P, R>(&self, uri: U, params: P) -> Result<R>
	where
		U: AsRef<str>,
		P: Serialize,
		R: DeserializeOwned;
}

pub trait Api: Config + Http {
	// fn uri_of<S>(&self, uri: S) -> String
	// where
	// 	S: AsRef<str>,
	// {
	// 	format!("{}/{}", self.base_uri(), uri.as_ref())
	// }

	async fn get_by_ticker<S>(&self, ticker: S) -> Result<Ticker>
	where
		S: AsRef<str>,
	{
		Ok(self
			.post::<_, _, Response<ResponseResult<Ticker>>>(
				// self.uri_of("blockchain.atomicals.get_by_ticker"),
				"blockchain.atomicals.get_by_ticker",
				Params::new([ticker.as_ref()]),
			)
			.await?
			.response
			.result)
	}

	async fn get_ft_info<S>(&self, atomical_id: S) -> Result<ResponseResult<Ft>>
	where
		S: AsRef<str>,
	{
		Ok(self
			.post::<_, _, Response<ResponseResult<Ft>>>(
				// self.uri_of("blockchain.atomicals.get_ft_info"),
				"blockchain.atomicals.get_ft_info",
				Params::new([atomical_id.as_ref()]),
			)
			.await?
			.response)
	}

	async fn get_unspent_address<S>(&self, address: S) -> Result<Vec<Utxo>>
	where
		S: AsRef<str>,
	{
		self.get_unspent_scripthash(util::address2scripthash(
			&Address::from_str(address.as_ref()).unwrap().require_network(*self.network())?,
		)?)
		.await
	}

	async fn get_unspent_scripthash<S>(&self, scripthash: S) -> Result<Vec<Utxo>>
	where
		S: AsRef<str>,
	{
		let mut utxos = self
			.post::<_, _, Response<Vec<Unspent>>>(
				// self.uri_of("blockchain.scripthash.listunspent"),
				"blockchain.scripthash.listunspent",
				Params::new([scripthash.as_ref()]),
			)
			.await?
			.response
			.into_iter()
			.map(|u| u.into())
			.collect::<Vec<Utxo>>();

		utxos.sort_by(|a, b| a.value.cmp(&b.value));

		Ok(utxos)
	}

	async fn wait_until_utxo<S>(&self, address: S, satoshis: u64) -> Result<Utxo>
	where
		S: AsRef<str>,
	{
		loop {
			for u in self.get_unspent_address(address.as_ref()).await? {
				if u.atomicals.is_empty() && u.value >= satoshis {
					return Ok(u);
				}
			}

			tracing::info!("waiting for UTXO...");

			time::sleep(Duration::from_secs(5)).await;
		}
	}

	async fn broadcast<S>(&self, tx: S) -> Result<String>
	where
		S: AsRef<str>,
	{
		Ok(self
			.post::<_, _, Response<String>>(
				"blockchain.transaction.broadcast",
				Params::new([tx.as_ref()]),
			)
			.await?
			.response)
	}
}
impl<T> Api for T where T: Config + Http {}

#[derive(Debug)]
pub struct ElectrumX {
	pub client: ReqwestClient,
	pub network: Network,
	pub base_uris: Vec<String>,
	pub max_retries: usize,
}
impl Config for ElectrumX {
	fn network(&self) -> &Network {
		&self.network
	}

	fn base_uris(&self) -> &[String] {
		&self.base_uris
	}
}
impl Http for ElectrumX {
	async fn post<U, P, R>(&self, endpoint: U, params: P) -> Result<R>
	where
		U: AsRef<str>,
		P: Serialize,
		R: DeserializeOwned,
	{
		let mut attempts = 0;
		let retry_delay = Duration::from_secs(2);
		let mut uri_index = 0;

		// TODO
		// 现在每次请求都是从 uri_index 0 开始，可以优化从上次成功的 URI 开始，需处理多线程的情况

		loop {
			let uri = format!("{}/{}", self.base_uris[uri_index], endpoint.as_ref());

			match self.client.post(&uri).json(&params).send().await {
				Ok(response) => {
					let resp_text = response.text().await?;
					match serde_json::from_str(&resp_text) {
						Ok(parsed) => return Ok(parsed),
						Err(e) => {
							tracing::info!("request {} parse response failed: {}", uri, e);
							// 解析失败时继续尝试
						},
					}
				},
				Err(e) => {
					tracing::info!("request {} failed: {}", uri, e);
					// 请求失败时继续尝试
				},
			}

			if attempts >= self.max_retries {
				if uri_index < self.base_uris.len() - 1 {
					uri_index += 1; // 切换到下一个 URI
					tracing::info!("switching to URI {}", self.base_uris[uri_index]);
					attempts = 0; // 重置尝试次数
				} else {
					return Err(anyhow::Error::msg("All URIs exhausted, still failed").into());
				}
			} else {
				attempts += 1;
				tracing::info!("retrying in {} seconds...", retry_delay.as_secs());
				tokio::time::sleep(retry_delay).await;
			}
		}
	}
}

#[derive(Debug)]
pub struct ElectrumXBuilder {
	pub network: Network,
	pub base_uris: Vec<String>,
}
impl ElectrumXBuilder {
	pub fn network(mut self, network: Network) -> Self {
		self.network = network;

		self
	}

	pub fn base_uris<S>(mut self, base_uris: S) -> Self
	where
		S: Into<String>,
	{
		self.base_uris = base_uris.into().split(',').map(String::from).collect();

		self
	}

	pub fn build(self) -> Result<ElectrumX> {
		Ok(ElectrumX {
			client: ReqwestClientBuilder::new().timeout(Duration::from_secs(30)).build()?,
			network: self.network,
			base_uris: self.base_uris,
			max_retries: 3, // 设置默认的重试次数
		})
	}
}
impl Default for ElectrumXBuilder {
	fn default() -> Self {
		Self { network: Network::Bitcoin, base_uris: vec!["https://ep.atomicals.xyz/proxy".into()] }
	}
}
