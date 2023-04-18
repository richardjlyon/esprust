use core::cell::RefCell;

use embedded_hal::blocking::i2c::{WriteRead, Read, Write};


/// A wrapper around refcell that implements WriteRead, Read and Write
pub struct RefCellBus<T>(RefCell<T>);

impl<T> RefCellBus<T> {
    pub fn new(val: T) -> Self {
        Self(RefCell::new(val))
    }
}

impl<T: WriteRead> WriteRead for & RefCellBus<T> {
    type Error = T::Error;

    fn write_read(
        &mut self,
        address: u8,
        bytes: &[u8],
        buffer: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.0.borrow_mut().write_read(address, bytes, buffer)
    }
}

impl<T: Read> Read for & RefCellBus<T> {
    type Error = T::Error;

    fn read(&mut self, address: u8, buffer: &mut [u8]) -> Result<(), Self::Error> {
        self.0.borrow_mut().read(address, buffer)
    }
}

impl<T: Write> Write for & RefCellBus<T> {
    type Error = T::Error;

    fn write(&mut self, address: u8, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0.borrow_mut().write(address, bytes)
    }
}