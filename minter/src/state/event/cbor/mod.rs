#[cfg(test)]
mod tests;

pub mod id {
    use minicbor::{
        Decode, Encode,
        decode::{Decoder, Error},
        encode::{Encoder, Write},
    };
    use phantom_newtype::Id;

    pub fn decode<'b, Ctx, Repr, Tag>(
        d: &mut Decoder<'b>,
        ctx: &mut Ctx,
    ) -> Result<Id<Tag, Repr>, Error>
    where
        Repr: Decode<'b, Ctx>,
    {
        Ok(Id::new(Repr::decode(d, ctx)?))
    }

    pub fn encode<Ctx, Repr, Tag, W: Write>(
        v: &Id<Tag, Repr>,
        e: &mut Encoder<W>,
        ctx: &mut Ctx,
    ) -> Result<(), minicbor::encode::Error<W::Error>>
    where
        Repr: Encode<Ctx>,
    {
        v.get().encode(e, ctx)
    }
}

pub mod signature {
    use minicbor::{
        decode::{Decoder, Error},
        encode::{Encoder, Write},
    };
    use solana_signature::Signature;

    pub fn decode<Ctx>(d: &mut Decoder<'_>, _ctx: &mut Ctx) -> Result<Signature, Error> {
        let bytes = d.bytes()?;
        Signature::try_from(bytes).map_err(|e| Error::message(e.to_string()))
    }

    pub fn encode<Ctx, W: Write>(
        v: &Signature,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.bytes(v.as_ref())?;
        Ok(())
    }
}

pub mod burn_index_signature_vec {
    use crate::numeric::LedgerBurnIndex;
    use minicbor::{
        decode::{Decoder, Error},
        encode::{Encoder, Write},
    };
    use solana_signature::Signature;

    pub fn decode<Ctx>(
        d: &mut Decoder<'_>,
        _ctx: &mut Ctx,
    ) -> Result<Vec<(LedgerBurnIndex, Signature)>, Error> {
        let len = d.array()?.ok_or_else(|| Error::message("expected definite-length array"))?;
        let mut result = Vec::with_capacity(len as usize);
        for _ in 0..len {
            d.array()?;
            let burn_index = LedgerBurnIndex::new(d.u64()?);
            let sig_bytes = d.bytes()?;
            let signature =
                Signature::try_from(sig_bytes).map_err(|e| Error::message(e.to_string()))?;
            result.push((burn_index, signature));
        }
        Ok(result)
    }

    pub fn encode<Ctx, W: Write>(
        v: &Vec<(LedgerBurnIndex, Signature)>,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.array(v.len() as u64)?;
        for (burn_index, signature) in v {
            e.array(2)?;
            e.u64(*burn_index.get())?;
            e.bytes(signature.as_ref())?;
        }
        Ok(())
    }
}

pub mod message {
    use minicbor::{
        decode::{Decoder, Error},
        encode::{Encoder, Write},
    };
    use solana_message::Message;

    pub fn decode<Ctx>(d: &mut Decoder<'_>, _ctx: &mut Ctx) -> Result<Message, Error> {
        let bytes = d.bytes()?;
        bincode::deserialize(bytes).map_err(|e| Error::message(e.to_string()))
    }

    pub fn encode<Ctx, W: Write>(
        v: &Message,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        let bytes = bincode::serialize(v)
            .map_err(|err| minicbor::encode::Error::message(err.to_string()))?;
        e.bytes(&bytes)?;
        Ok(())
    }
}
