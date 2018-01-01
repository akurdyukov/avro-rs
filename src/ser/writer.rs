use std::collections::HashMap;
use std::io::Write;
use std::iter::once;
use std::rc::Rc;

use failure::{Error, err_msg};
use rand::random;
use serde::Serialize;
use serde_json;

use Codec;
use ser::encode::EncodeAvro;
use schema::{Name, Schema};
use ser::ser::Serializer;
use types::{ToAvro, Value};

pub struct Writer<'a, W> {
    schema: &'a Schema,
    serializer: Serializer<'a>,
    writer: W,
    codec: Codec,
    marker: Vec<u8>,
    has_header: bool,
}

impl<'a, W: Write> Writer<'a, W> {
    pub fn new(schema: &'a Schema, writer: W) -> Writer<'a, W> {
        Self::with_codec(schema, writer, Codec::Null)
    }

    pub fn with_codec(schema: &'a Schema, writer: W, codec: Codec) -> Writer<'a, W> {
        let mut marker = Vec::with_capacity(16);
        for _ in 0..16 {
            marker.push(random::<u8>());
        }

        Writer {
            schema: schema,
            serializer: Serializer::new(schema),
            writer: writer,
            codec: codec,
            marker: marker,
            has_header: false,
        }
    }

    pub fn schema(&self) -> &'a Schema {
        self.schema
    }

    pub fn header(&mut self) -> Result<usize, Error> {
        let magic_schema = Schema::Fixed { name: Name::new("Magic"), size: 4 };
        let meta_schema = &Schema::Map(Rc::new(Schema::Bytes));
        let mut metadata = HashMap::new();
        metadata.insert("avro.schema", Value::Bytes(serde_json::to_string(self.schema)?.into_bytes()));
        metadata.insert("avro.codec", self.codec.avro());

        Ok(self.append_raw(&magic_schema, &['O' as u8, 'b' as u8, 'j' as u8, 1u8][..])? +
               self.append_raw(&meta_schema, metadata.avro())? +
               self.append_marker()?)
    }

    pub fn append<S: Serialize>(&mut self, value: S) -> Result<usize, Error> {
        self.extend(once(value))
    }

    fn append_marker(&mut self) -> Result<usize, Error> {
        // using .writer.write directly to avoid mutable borrow of self
        // with ref borrowing of self.marker
        Ok(self.writer.write(&self.marker)?)
    }

    fn append_raw<V>(&mut self, schema: &Schema, value: V) -> Result<usize, Error> where V: EncodeAvro {
        match value.encode(schema) {
            Some(stream) => Ok(self.writer.write(stream.as_ref())?),
            None => Err(err_msg("value does not match given schema")),
        }
    }

    pub fn extend<I, S: Serialize>(&mut self, values: I) -> Result<usize, Error>
        where I: Iterator<Item=S>
    {
        let mut num_values = 0;
        /*
        https://github.com/rust-lang/rfcs/issues/811 :(
        let mut stream = values
            .filter_map(|value| value.serialize(&mut self.serializer).ok())  // TODO not filter
            .map(|value| value.encode(self.schema))  // TODO with_schema
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| err_msg("value does not match given schema"))?
            .into_iter()
            .fold(Vec::new(), |mut acc, stream| {
                num_values += 1;
                acc.extend(stream); acc
            });
        */

        let mut v = Vec::new();
        for value in values {
            match value.serialize(&mut self.serializer) {
                Ok(s) => match s.encode(self.schema) {
                    Some(stream) => {
                        v.push(stream);
                        num_values += 1;
                    },
                    None => return Err(err_msg("value does not match schema")),
                },
                Err(e) => Err(e)?,
            }
        }

        let mut stream: Vec<u8> = v.iter()
            .fold(Vec::new(), |mut acc, s| { acc.extend(s); acc });

        stream = self.codec.compress(stream)?;

        if !self.has_header {
            self.header()?;
            self.has_header = true;
        }

        Ok(self.append_raw(&Schema::Long, num_values)? +
            self.append_raw(&Schema::Long, stream.len())? +
            self.writer.write(stream.as_ref())? +
            self.append_marker()?)
    }

    pub fn into_inner(self) -> W {
        self.writer
    }
}