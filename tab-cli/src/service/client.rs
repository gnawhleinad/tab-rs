use crate::bus::ClientBus;
use crate::{
    message::{
        client::TabTerminated,
        main::MainShutdown,
        tabs::TabsRecv,
        terminal::{TerminalRecv, TerminalSend},
    },
    state::{
        tab::{TabState, TabStateAvailable},
        terminal::TerminalSizeState,
    },
};

use anyhow::Context;

use tab_api::{
    chunk::InputChunk,
    request::Request,
    response::Response,
    tab::{CreateTabMetadata, TabMetadata},
};
use tab_service::{Bus, Lifeline, Service};

pub struct ClientService {
    _request_tab: Lifeline,
    _websocket: WebsocketMessageService,
    _terminal: TerminalMessageService,
}
impl Service for ClientService {
    type Bus = ClientBus;
    type Lifeline = anyhow::Result<Self>;

    fn spawn(bus: &ClientBus) -> anyhow::Result<Self> {
        let _request_tab = {
            let mut rx_tab_state = bus.rx::<TabState>()?;
            let rx_terminal_size = bus.rx::<TerminalSizeState>()?;
            let mut tx_request = bus.tx::<Request>()?;

            Self::try_task("request_tab", async move {
                while let Some(update) = rx_tab_state.recv().await {
                    if let TabState::Awaiting(name) = update {
                        let dimensions = rx_terminal_size.borrow().clone().0;
                        tx_request
                            .send(Request::CreateTab(CreateTabMetadata { name, dimensions }))
                            .await?;
                    }
                }

                Ok(())
            })
        };

        // let websocket_rx = WebsocketMessageRx {
        //     websocket: rx.websocket,
        //     tab_state: rx.tab_state.clone(),
        //     terminal_size: rx.terminal_size,
        // };

        // let websocket_tx = WebsocketMessageTx {
        //     websocket: tx.websocket.clone(),
        //     terminal: tx.terminal.clone(),
        //     active_tabs: tx.active_tabs,
        //     tab_metadata: tx.tab_metadata,
        //     shutdown: tx.shutdown,
        // };
        let _websocket = WebsocketMessageService::spawn(bus)?;

        // let terminal_rx = TerminalMessageRx {
        //     terminal: rx.terminal,
        //     tab_state: rx.tab_state.clone(),
        // };
        // let terminal_tx = TerminalMessageTx {
        //     websocket: tx.websocket,
        // };
        let _terminal = TerminalMessageService::spawn(bus)?;

        Ok(ClientService {
            _request_tab,
            _websocket,
            _terminal,
        })
    }
}

struct WebsocketMessageService {
    _websocket: Lifeline,
}

impl Service for WebsocketMessageService {
    type Bus = ClientBus;
    type Lifeline = anyhow::Result<Self>;

    fn spawn(bus: &ClientBus) -> anyhow::Result<Self> {
        let mut rx_websocket = bus.rx::<Response>()?;
        let rx_tab_state = bus.rx::<TabState>()?;

        let tx_terminal = bus.tx::<TerminalRecv>()?;
        let tx_tab_metadata = bus.tx::<TabMetadata>()?;
        let tx_available_tabs = bus.tx::<TabStateAvailable>()?;
        let mut tx_shutdown = bus.tx::<MainShutdown>()?;
        let mut tx_tabs = bus.tx::<TabsRecv>()?;
        let mut tx_tab_terminated = bus.tx::<TabTerminated>()?;

        let _websocket = Self::try_task("recv", async move {
            while let Some(msg) = rx_websocket.recv().await {
                match msg {
                    Response::Init(init) => {
                        tx_tabs.send(TabsRecv::Init(init.tabs.clone())).await?;

                        for (_id, metadata) in init.tabs {
                            tx_tab_metadata
                                .send(metadata)
                                .map_err(|_| anyhow::Error::msg("send TabMetadata"))?;
                        }
                    }
                    Response::Output(tab_id, stdout) => {
                        if rx_tab_state.borrow().is_selected(&tab_id) {
                            tx_terminal
                                .send(TerminalRecv::Stdout(stdout.data))
                                .map_err(|_e| anyhow::Error::msg("tx TerminalRecv::Stdout"))?;
                        }
                    }
                    Response::TabUpdate(tab) => {
                        tx_tab_metadata
                            .send(tab.clone())
                            .map_err(|_| anyhow::Error::msg("send TabMetadata"))?;

                        tx_tabs.send(TabsRecv::Update(tab)).await?;
                    }
                    Response::TabList(tabs) => {
                        tx_available_tabs.broadcast(TabStateAvailable(tabs))?
                    }
                    Response::TabTerminated(id) => {
                        tx_tabs.send(TabsRecv::Terminated(id)).await?;

                        tx_tab_terminated.send(TabTerminated(id)).await?;

                        if rx_tab_state.borrow().is_selected(&id) {
                            tx_shutdown
                                .send(MainShutdown {})
                                .await
                                .map_err(|_| anyhow::Error::msg("send MainShutdown"))?;
                        }
                    }
                }
            }

            Ok(())
        });

        Ok(Self { _websocket })
    }
}

impl WebsocketMessageService {}

struct TerminalMessageService {
    _terminal: Lifeline,
}

impl Service for TerminalMessageService {
    type Bus = ClientBus;
    type Lifeline = anyhow::Result<Self>;

    fn spawn(bus: &ClientBus) -> anyhow::Result<Self> {
        let mut rx = bus.rx::<TerminalSend>()?;
        let rx_tab_state = bus.rx::<TabState>()?;

        let mut tx = bus.tx::<Request>()?;

        let _terminal = Self::try_task("terminal", async move {
            // TODO: replace with better error handling for the broadcast channel
            while let Ok(msg) = rx.recv().await {
                let tab_state = rx_tab_state.borrow().clone();
                match (tab_state, msg) {
                    (TabState::Selected(id, _), TerminalSend::Stdin(data)) => {
                        let request = Request::Input(id, InputChunk { data });
                        tx.send(request).await.context("Request send")?
                    }
                    _ => {}
                }
            }

            Ok(())
        });

        Ok(Self { _terminal })
    }
}
