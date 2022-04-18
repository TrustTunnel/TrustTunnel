use std::fmt::Debug;
use std::io;
use async_trait::async_trait;
use futures::future;
use crate::{log_id, log_utils, pipe};


/// An abstract interface for a datagram receiver implementation
#[async_trait]
pub(crate) trait Source: Send {
    type Output;

    fn id(&self) -> log_utils::IdChain<u64>;

    /// Listen for an incoming datagram
    async fn read(&mut self) -> io::Result<Self::Output>;
}

pub(crate) enum SendStatus {
    /// A sink sent the full chunk successfully
    Sent,
    /// A sink did not send anything as it is not able to send the full chunk at the moment
    /// (for example, due to flow control limits)
    Dropped,
}

/// An abstract interface for a datagram transmitter implementation
#[async_trait]
pub(crate) trait Sink: Send {
    type Input;

    /// Send a data chunk to the peer.
    ///
    /// # Return
    ///
    /// See [`SendStatus`]
    async fn write(&mut self, data: Self::Input) -> io::Result<SendStatus>;
}


/// An abstract interface for a two-way datagram channel implementation
#[async_trait]
pub(crate) trait DuplexPipe: Send {
    /// Exchange datagrams until some error happened or the channel is closed
    async fn exchange(&mut self) -> io::Result<()>;
}

pub(crate) struct GenericSimplexPipe<D: Debug> {
    direction: pipe::SimplexPipeDirection,
    source: Box<dyn Source<Output = D>>,
    sink: Box<dyn Sink<Input = D>>,
}

pub(crate) struct GenericDuplexPipe<D1: Debug, D2: Debug> {
    left_pipe: GenericSimplexPipe<D1>,
    right_pipe: GenericSimplexPipe<D2>,
}

impl<D1: Debug, D2: Debug> GenericDuplexPipe<D1, D2> {
    pub fn new(
        left_pipe: GenericSimplexPipe<D1>,
        right_pipe: GenericSimplexPipe<D2>,
    ) -> Self {
        Self {
            left_pipe,
            right_pipe,
        }
    }
}

#[async_trait]
impl<D1: Send + Debug, D2: Send + Debug> DuplexPipe for GenericDuplexPipe<D1, D2> {
    async fn exchange(&mut self) -> io::Result<()> {
        let left = self.left_pipe.exchange();
        futures::pin_mut!(left);
        let right = self.right_pipe.exchange();
        futures::pin_mut!(right);
        match future::try_select(left, right).await {
            Ok(_) => Ok(()),
            Err(future::Either::Left((e, _))) | Err(future::Either::Right((e, _))) => Err(e),
        }
    }
}

impl<D: Debug> GenericSimplexPipe<D> {
    pub fn new(
        direction: pipe::SimplexPipeDirection,
        source: Box<dyn Source<Output = D>>,
        sink: Box<dyn Sink<Input = D>>,
    ) -> Self {
        Self {
            direction,
            source,
            sink,
        }
    }

    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            let datagram = self.source.read().await?;
            log_id!(trace, self.source.id(), "{} Datagram: {:?}", self.direction, datagram);

            match self.sink.write(datagram).await? {
                SendStatus::Sent => log_id!(debug, self.source.id(), "{} Datagram sent", self.direction),
                SendStatus::Dropped => log_id!(debug, self.source.id(), "{} Datagram dropped", self.direction),
            }
        }
    }
}
