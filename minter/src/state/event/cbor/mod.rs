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

pub mod mint_indices {
    use crate::numeric::LedgerMintIndex;
    use minicbor::{
        decode::{Decoder, Error},
        encode::{Encoder, Write},
    };

    pub fn decode<Ctx>(d: &mut Decoder<'_>, _ctx: &mut Ctx) -> Result<Vec<LedgerMintIndex>, Error> {
        let len = d
            .array()?
            .ok_or_else(|| Error::message("expected definite array"))?;
        let mut indices = Vec::with_capacity(len as usize);
        for _ in 0..len {
            let val: u64 = d.u64()?;
            indices.push(LedgerMintIndex::from(val));
        }
        Ok(indices)
    }

    pub fn encode<Ctx, W: Write>(
        v: &Vec<LedgerMintIndex>,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.array(v.len() as u64)?;
        for idx in v {
            e.u64(*idx.get())?;
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
