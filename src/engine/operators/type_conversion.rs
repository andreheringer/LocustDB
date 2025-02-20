use ordered_float::OrderedFloat;

use crate::engine::*;
use crate::mem_store::Val;


#[derive(Debug)]
pub struct TypeConversionOperator<T, U> {
    pub input: BufferRef<T>,
    pub output: BufferRef<U>,
}

impl<'a, T: 'a, U: 'a> VecOperator<'a> for TypeConversionOperator<T, U> where
    T: VecData<T> + Copy, U: VecData<U>, T: Cast<U> {
    fn execute(&mut self, stream: bool, scratchpad: &mut Scratchpad<'a>) -> Result<(), QueryError>{
        let data = scratchpad.get(self.input);
        let mut output = scratchpad.get_mut(self.output);
        if stream { output.clear() }
        for d in data.iter() {
            let casted = Cast::<U>::cast(*d);
            output.push(casted);
        }
        Ok(())
    }

    fn init(&mut self, _: usize, batch_size: usize, scratchpad: &mut Scratchpad<'a>) {
        scratchpad.set(self.output, Vec::with_capacity(batch_size));
    }

    fn inputs(&self) -> Vec<BufferRef<Any>> { vec![self.input.any()] }
    fn outputs(&self) -> Vec<BufferRef<Any>> { vec![self.output.any()] }
    fn can_stream_input(&self, _: usize) -> bool { true }
    fn can_stream_output(&self, _: usize) -> bool { true }
    fn allocates(&self) -> bool { true }

    fn display_op(&self, _: bool) -> String {
        format!("{} as {:?}", self.input, U::t())
    }
}


pub trait Cast<T> {
    fn cast(self) -> T;
}

impl<T> Cast<T> for T { fn cast(self) -> T { self } }


impl Cast<u8> for u16 { fn cast(self) -> u8 { self as u8 } }

impl Cast<u8> for u32 { fn cast(self) -> u8 { self as u8 } }

impl Cast<u8> for u64 { fn cast(self) -> u8 { self as u8 } }

impl Cast<u8> for i64 { fn cast(self) -> u8 { self as u8 } }


impl Cast<u16> for u8 { fn cast(self) -> u16 { u16::from(self) } }

impl Cast<u16> for u32 { fn cast(self) -> u16 { self as u16 } }

impl Cast<u16> for u64 { fn cast(self) -> u16 { self as u16 } }

impl Cast<u16> for i64 { fn cast(self) -> u16 { self as u16 } }


impl Cast<u32> for u8 { fn cast(self) -> u32 { u32::from(self) } }

impl Cast<u32> for u16 { fn cast(self) -> u32 { u32::from(self) } }

impl Cast<u32> for u64 { fn cast(self) -> u32 { self as u32 } }

impl Cast<u32> for i64 { fn cast(self) -> u32 { self as u32 } }


impl Cast<i64> for u8 { fn cast(self) -> i64 { i64::from(self) } }

impl Cast<i64> for u16 { fn cast(self) -> i64 { i64::from(self) } }

impl Cast<i64> for u32 { fn cast(self) -> i64 { i64::from(self) } }

impl Cast<i64> for u64 { fn cast(self) -> i64 { self as i64 } }


impl Cast<u64> for u8 { fn cast(self) -> u64 { u64::from(self) } }

impl Cast<u64> for u16 { fn cast(self) -> u64 { u64::from(self) } }

impl Cast<u64> for u32 { fn cast(self) -> u64 { u64::from(self) } }

impl Cast<u64> for i64 { fn cast(self) -> u64 { self as u64 } }


impl<'a> Cast<Val<'a>> for u8 { fn cast(self) -> Val<'a> { Val::Integer(self as i64) } }

impl<'a> Cast<Val<'a>> for u16 { fn cast(self) -> Val<'a> { Val::Integer(self as i64) } }

impl<'a> Cast<Val<'a>> for u32 { fn cast(self) -> Val<'a> { Val::Integer(self as i64) } }

impl<'a> Cast<Val<'a>> for i64 { fn cast(self) -> Val<'a> { Val::Integer(self) } }

impl<'a> Cast<Val<'a>> for &'a str { fn cast(self) -> Val<'a> { Val::Str(self) } }

impl<'a> Cast<Val<'a>> for OrderedFloat<f64> { fn cast(self) -> Val<'a> { Val::Float(self) } }

impl<'a> Cast<u8> for Val<'a> {
    fn cast(self) -> u8 {
        match self {
            Val::Integer(i) => i as u8,
            _ => panic!("Cast::<u8>{:?}", self)
        }
    }
}

impl<'a> Cast<u16> for Val<'a> {
    fn cast(self) -> u16 {
        match self {
            Val::Integer(i) => i as u16,
            _ => panic!("Cast::<u16>{:?}", self)
        }
    }
}

impl<'a> Cast<u32> for Val<'a> {
    fn cast(self) -> u32 {
        match self {
            Val::Integer(i) => i as u32,
            _ => panic!("Cast::<u32>{:?}", self)
        }
    }
}

impl<'a> Cast<i64> for Val<'a> {
    fn cast(self) -> i64 {
        match self {
            Val::Integer(i) => i,
            _ => panic!("Cast::<i64>{:?}", self)
        }
    }
}

impl<'a> Cast<OrderedFloat<f64>> for Val<'a> {
    fn cast(self) -> OrderedFloat<f64> {
        match self {
            Val::Float(f) => f,
            _ => panic!("Cast::<f64>{:?}", self)
        }
    }
}

impl<'a> Cast<&'a str> for Val<'a> {
    fn cast(self) -> &'a str {
        match self {
            Val::Str(s) => s,
            _ => panic!("Cast::<&str>{:?}", self)
        }
    }
}

impl<'a> Cast<Option<&'a str>> for &'a str { fn cast(self) -> Option<&'a str> { Some(self) } }