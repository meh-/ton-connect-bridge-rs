use crate::{
    message_courier::MessageCourier,
    models::TonEvent,
    storage::{EventStorage, EventStorageError},
};
use mockall::mock;
use tokio::sync::mpsc::UnboundedReceiver;

mock! {
    #[derive(Clone)]
    pub Storage {}
    impl EventStorage for Storage {
        fn add(
            &self,
            event: TonEvent,
        ) -> impl std::future::Future<Output = Result<(), EventStorageError>> + Send;
        fn get_since(
            &self,
            client_id: &String,
            event_id: &String,
        ) -> impl std::future::Future<Output = Result<Vec<TonEvent>, EventStorageError>> + Send;
    }
    impl Clone for Storage {
        fn clone(&self) -> Self;
    }
}

mock! {
    #[derive(Clone)]
    pub Courier {}
    impl MessageCourier for Courier {
        fn register_client(&self, client_id: String) -> UnboundedReceiver<TonEvent>;
        fn start(self, channel: &str);
    }
    impl Clone for Courier {
        fn clone(&self) -> Self;
    }
}
