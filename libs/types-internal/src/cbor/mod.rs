use minicbor::{
    data::Type,
    decode::{Decoder, Error},
    encode::{Encoder, Write},
};

#[cfg(test)]
mod tests;

pub mod u128 {
    use super::*;

    pub fn encode<Ctx, W: Write>(
        v: &u128,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.bytes(&v.to_be_bytes())?;
        Ok(())
    }

    pub fn decode<Ctx>(d: &mut Decoder<'_>, _ctx: &mut Ctx) -> Result<u128, Error> {
        let bytes = d.bytes()?;
        <[u8; 16]>::try_from(bytes)
            .map(u128::from_be_bytes)
            .map_err(|e| Error::message(e.to_string()))
    }

    pub mod option {
        use super::*;

        pub fn encode<Ctx, W: Write>(
            val: &Option<u128>,
            e: &mut Encoder<W>,
            ctx: &mut Ctx,
        ) -> Result<(), minicbor::encode::Error<W::Error>> {
            match val {
                Some(v) => super::encode(v, e, ctx),
                None => {
                    e.null()?;
                    Ok(())
                }
            }
        }

        pub fn decode<Ctx>(d: &mut Decoder<'_>, ctx: &mut Ctx) -> Result<Option<u128>, Error> {
            if d.datatype()? == Type::Null {
                d.null()?;
                Ok(None)
            } else {
                super::decode(d, ctx).map(Some)
            }
        }
    }
}
