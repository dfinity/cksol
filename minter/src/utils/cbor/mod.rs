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

    pub mod option {
        use super::*;

        pub fn decode<Ctx>(d: &mut Decoder<'_>, ctx: &mut Ctx) -> Result<Option<Signature>, Error> {
            if d.datatype()? == minicbor::data::Type::Null {
                d.null()?;
                return Ok(None);
            }
            super::decode(d, ctx).map(Some)
        }

        pub fn encode<Ctx, W: Write>(
            v: &Option<Signature>,
            e: &mut Encoder<W>,
            ctx: &mut Ctx,
        ) -> Result<(), minicbor::encode::Error<W::Error>> {
            match v {
                None => {
                    e.null()?;
                    Ok(())
                }
                Some(sig) => super::encode(sig, e, ctx),
            }
        }
    }
}

pub mod id_vec {
    use minicbor::{
        decode::{Decoder, Error},
        encode::{Encoder, Write},
    };
    use phantom_newtype::Id;

    pub fn decode<Ctx, Tag>(
        d: &mut Decoder<'_>,
        _ctx: &mut Ctx,
    ) -> Result<Vec<Id<Tag, u64>>, Error> {
        let len = d
            .array()?
            .ok_or_else(|| Error::message("expected definite array"))?;
        let mut indices = Vec::with_capacity(len as usize);
        for _ in 0..len {
            indices.push(Id::new(d.u64()?));
        }
        Ok(indices)
    }

    pub fn encode<Ctx, Tag, W: Write>(
        v: &Vec<Id<Tag, u64>>,
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
