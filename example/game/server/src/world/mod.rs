//! Game world: baked map data, hub channels, mission lobbies, stage
//! instances and their wire protocol.

pub mod directory;
pub mod hub;
pub mod lobby;
pub mod map;
pub mod missions;
pub mod protocol;
pub mod stage;

pub use directory::{HubCommand, HubConfig, HubDirectory, HubTicket, JoinError, OUTBOX_CAPACITY};
pub use hub::{Arrival, HubEvent, HubInstance, HubSimConfig, HubWorld, InstanceKind, PlayerProfile, PortalLink};
pub use lobby::{LobbyDirectory, LobbyError, LobbyEvent, LOBBY_CHANNEL_CAPACITY};
pub use stage::{StageConfig, StageDirectory, StageError, StageTicket};
pub use map::{CameraConstraint, MapData, MapError, NpcDef};
pub use missions::MissionInfo;
pub use protocol::{ClientMsg, ServerMsg, Snapshot};
