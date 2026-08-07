use crate::{log_id, log_utils, pipe};
use async_trait::async_trait;
use futures::future;
use std::fmt::Debug;
use std::io;

pub(crate) trait Datagram {
    /// Get on-the-wire length of the datagram
    fn len(&self) -> usize;
}

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

pub(crate) struct GenericSimplexPipe<D, F> {
    direction: pipe::SimplexDirection,
    source: Box<dyn Source<Output = D>>,
    sink: Box<dyn Sink<Input = D>>,
    update_metrics: F,
}

pub(crate) struct GenericDuplexPipe<D1, D2, F> {
    left_pipe: GenericSimplexPipe<D1, F>,
    right_pipe: GenericSimplexPipe<D2, F>,
}

impl<D1, D2, F> GenericDuplexPipe<D1, D2, F>
where
    D1: Datagram + Debug,
    D2: Datagram + Debug,
    F: Fn(pipe::SimplexDirection, usize, log_utils::IdChain<u64>) + Send + Clone,
{
    pub fn new(
        (dir1, source1, sink1): (
            pipe::SimplexDirection,
            Box<dyn Source<Output = D1>>,
            Box<dyn Sink<Input = D1>>,
        ),
        (dir2, source2, sink2): (
            pipe::SimplexDirection,
            Box<dyn Source<Output = D2>>,
            Box<dyn Sink<Input = D2>>,
        ),
        update_metrics: F,
    ) -> Self {
        Self {
            left_pipe: GenericSimplexPipe::new(dir1, source1, sink1, update_metrics.clone()),
            right_pipe: GenericSimplexPipe::new(dir2, source2, sink2, update_metrics),
        }
    }
}

#[async_trait]
impl<D1, D2, F> DuplexPipe for GenericDuplexPipe<D1, D2, F>
where
    D1: Datagram + Send + Debug,
    D2: Datagram + Send + Debug,
    F: Fn(pipe::SimplexDirection, usize, log_utils::IdChain<u64>) + Send,
{
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

impl<D: Datagram + Debug, F: Fn(pipe::SimplexDirection, usize) + Send> GenericSimplexPipe<D, F> {
    pub fn new(
        direction: pipe::SimplexDirection,
        source: Box<dyn Source<Output = D>>,
        sink: Box<dyn Sink<Input = D>>,
        update_metrics: F,
    ) -> Self {
        Self {
            direction,
            source,
            sink,
            update_metrics,
        }
    }

    async fn exchange(&mut self) -> io::Result<()> {
        loop {
            let datagram = self.source.read().await?;
            log_id!(
                trace,
                self.source.id(),
                "{} Datagram: {:?}",
                self.direction,
                datagram
            );

            let datagram_len = datagram.len();
            match self.sink.write(datagram).await? {
                SendStatus::Sent => {
                    (self.update_metrics)(self.direction, datagram_len, self.source.id());
                }
                SendStatus::Dropped => log_id!(
                    trace,
                    self.source.id(),
                    "{} Datagram dropped",
                    self.direction
                ),
            }
        }
    }
}
