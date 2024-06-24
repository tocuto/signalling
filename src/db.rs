use std::{collections::HashMap, marker::PhantomData};

use rand::{rngs::SmallRng, seq::SliceRandom, Rng, SeedableRng};
use serde::{de::DeserializeOwned, Serialize};
use web_time::{SystemTime, UNIX_EPOCH};
use worker::{Bucket, Object, Result};

fn random_string(rng: &mut impl Rng, len: u8) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

    (0..len)
        .map(|_| *ALPHABET.choose(rng).unwrap() as char)
        .collect()
}

pub trait Metadata: From<HashMap<String, String>> + Into<HashMap<String, String>> {}

pub trait BucketInfo {
    const PREFIX: &'static str = "";
    const KEY_LENGTH: u8 = 0;
}

pub struct Data<O, M, B> {
    pub modified: bool,
    pub key: String,
    pub data: Option<O>,
    pub meta: M,
    info: PhantomData<B>,
}

impl<O, M, B> Data<O, M, B>
where
    O: Serialize + DeserializeOwned + Default,
    M: Metadata + Default,
    B: BucketInfo,
{
    pub fn get_bucket_key(key: &str) -> String {
        format!("{}:{}", B::PREFIX, key)
    }

    fn remove_prefix(key: String) -> String {
        key.get((B::PREFIX.len() + 1)..)
            .expect("invalid key")
            .to_owned()
    }

    async fn new_key(bucket: &Bucket) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time travel?")
            .as_secs();
        let mut rng = SmallRng::seed_from_u64(now);

        loop {
            let key = random_string(&mut rng, B::KEY_LENGTH);

            // Retry if object already exists
            let obj = bucket.head(Self::get_bucket_key(&key)).await?;
            if obj.is_none() {
                return Ok(key);
            }
        }
    }

    pub async fn create(bucket: &Bucket) -> Result<Self> {
        let key = Self::new_key(bucket).await?;
        Ok(Self {
            modified: true,
            key,
            data: Some(Default::default()),
            meta: Default::default(),
            info: PhantomData,
        })
    }

    pub async fn load(bucket: &Bucket, key: &str) -> Result<Option<Self>> {
        match bucket.get(Self::get_bucket_key(key)).execute().await? {
            Some(obj) => Ok(Some(Self::_read(key.to_owned(), &obj).await?)),
            None => Ok(None),
        }
    }

    async fn _read(key: String, obj: &Object) -> Result<Self> {
        let meta: M = obj.custom_metadata()?.into();
        let data = match obj.body() {
            Some(b) => Some(b.bytes().await?),
            None => None,
        };
        let data = data.map(|d| serde_bare::de::from_slice(&d).unwrap());

        Ok(Self {
            modified: false,
            key,
            data,
            meta,
            info: PhantomData,
        })
    }

    pub async fn read(obj: &Object) -> Result<Self> {
        Self::_read(Self::remove_prefix(obj.key()), obj).await
    }

    pub async fn write(self, bucket: &Bucket) -> Result<()> {
        if !self.modified {
            return Ok(());
        }

        let key = Self::get_bucket_key(&self.key);
        let data = self.data.as_ref().unwrap();
        bucket
            .put(key, serde_bare::ser::to_vec(data).unwrap())
            .custom_metadata(self.meta)
            .execute()
            .await?;
        Ok(())
    }
}
