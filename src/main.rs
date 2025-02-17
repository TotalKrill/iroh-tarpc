use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use iroh::Endpoint;
use iroh::protocol::{ProtocolHandler, Router};

use tarpc::server::{self, Channel as _};
use tarpc::tokio_serde::formats::Bincode;
use tarpc::tokio_util::codec::LengthDelimitedCodec;
use tarpc::{client, context};

/// A way to implement a type that has both AsyncRead and AsyncWrite from the separate SendStream, RecvStream
/// received from the
mod duplex_stream;
use duplex_stream::Duplex;

use tokio::sync::Mutex;

#[tarpc::service]
pub trait HelloWorld {
    /// yep
    async fn hello(who: String) -> String;
    /// returns how many responses this server has done
    async fn amount_responses() -> usize;
}

pub const HELLOWORLD_ALPN: &[u8] = b"HELLOWORLD_ALPN";

#[derive(Debug, Clone)]
pub struct HelloWorldServer {
    amount: Arc<Mutex<usize>>,
}

impl HelloWorld for HelloWorldServer {
    async fn hello(self, _context: ::tarpc::context::Context, who: std::string::String) -> String {
        let mut amount = self.amount.lock().await;
        *amount += 1;
        format!("Hello {who}")
    }

    async fn amount_responses(self, _context: ::tarpc::context::Context) -> usize {
        let amount = self.amount.lock().await;
        *amount
    }
}

impl ProtocolHandler for HelloWorldServer {
    fn accept(
        &self,
        conn: iroh::endpoint::Connecting,
    ) -> futures_lite::future::Boxed<anyhow::Result<()>> {
        let clone = self.clone();
        Box::pin(async move {
            let connection = conn.await?;
            let conn = connection.accept_bi().await?;
            let duplex = Duplex::new(conn.1, conn.0);

            let codec_builder = LengthDelimitedCodec::builder();
            let framed = codec_builder.new_framed(duplex);

            let server_transport = tarpc::serde_transport::new(framed, Bincode::default());
            let server = server::BaseChannel::with_defaults(server_transport);

            server
                .execute(clone.serve())
                // Handle all requests concurrently.
                .for_each(|response| async move {
                    tokio::spawn(response);
                })
                .await;

            Ok(())
        })
    }
}

#[derive(Clone)]
pub struct HelloClient {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // start a server
    let server_endpoint = Endpoint::builder()
        .discovery_n0()
        .alpns(vec![HELLOWORLD_ALPN.to_vec()])
        .bind()
        .await?;

    let server = HelloWorldServer {
        amount: Arc::new(Mutex::new(0)),
    };
    let server_node = server_endpoint.node_addr().await?;
    let _relay = Router::builder(server_endpoint)
        .accept(HELLOWORLD_ALPN, std::sync::Arc::new(server))
        .spawn()
        .await?;

    // start up a client
    let client_endpoint = Endpoint::builder().discovery_n0().bind().await?;
    println!("client: {}", client_endpoint.node_id().fmt_short());

    let (sendstream, recvstream) = client_endpoint
        .connect(server_node, HELLOWORLD_ALPN)
        .await?
        .open_bi()
        .await?;
    let duplex = Duplex::new(recvstream, sendstream);

    // WorldClient is generated by the #[tarpc::service] attribute. It has a constructor `new`
    // that takes a config and any Transport as input.
    let codec_builder = LengthDelimitedCodec::builder();
    let framed = codec_builder.new_framed(duplex);

    let client_transport = tarpc::serde_transport::new(framed, Bincode::default());
    let client = HelloWorldClient::new(client::Config::default(), client_transport).spawn();

    // The client has an RPC method for each RPC defined in the annotated trait. It takes the same
    // args as defined, with the addition of a Context, which is always the first arg. The Context
    // specifies a deadline and trace information which can be helpful in debugging requests.
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let hello = client.hello(context::current(), "Hot stuff".into()).await?;

        let amount = client.amount_responses(context::current()).await?;
        println!("{hello}: {amount}");
    }
}
