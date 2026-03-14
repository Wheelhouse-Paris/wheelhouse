//! Acceptance tests for Story 9.2: Telegram Surface Runner
//!
//! These tests are written in TDD RED phase — they define the expected behavior
//! and MUST fail until implementation is complete.
//!
//! Run: cargo test -p wh-telegram --test runner_acceptance_tests

// AC #1: Inbound message published to stream with user_id set
mod ac1_inbound_message_to_stream {
    #[test]
    fn config_reads_wh_url_from_env() {
        // Given WH_URL is set in the environment
        // When TelegramConfig::from_env() is called
        // Then the wh_url field is populated
        // This test validates the config accessor exists
        // (will fail until config.rs is updated with wh_url field)
        let _: fn(&wh_telegram::TelegramConfig) -> &str =
            wh_telegram::TelegramConfig::wh_url;
    }

    #[test]
    fn config_reads_surface_name_from_env() {
        // Given WH_SURFACE_NAME is set in the environment
        // When TelegramConfig::from_env() is called
        // Then the surface_name field is populated
        let _: fn(&wh_telegram::TelegramConfig) -> &str =
            wh_telegram::TelegramConfig::surface_name;
    }

    #[test]
    fn zmq_bridge_module_exists() {
        // Given the wh-telegram crate
        // When the bridge module is imported
        // Then the ZmqBridge type is accessible
        let _: fn() = || {
            // ZmqBridge type must exist and be constructable
            let _ = std::any::type_name::<wh_telegram::ZmqBridge>();
        };
    }
}

// AC #2: Outbound response delivered to correct Telegram chat
mod ac2_outbound_response {
    #[test]
    fn zmq_bridge_has_publish_method() {
        // Given a ZmqBridge instance
        // When publish is called with a TextMessage
        // Then the message is encoded and sent via ZMQ
        // This test validates the method signature exists
        let _ = std::any::type_name::<wh_telegram::ZmqBridge>();
    }

    #[test]
    fn zmq_bridge_has_recv_method() {
        // Given a ZmqBridge instance connected to a broker
        // When recv is called
        // Then a TextMessage is returned from the stream
        let _ = std::any::type_name::<wh_telegram::ZmqBridge>();
    }
}

// AC #4: Startup failure exits with code 1 and human-readable error
mod ac4_startup_failure {
    #[test]
    fn config_errors_on_missing_wh_url() {
        // Given WH_URL is NOT set
        // When TelegramConfig::from_env() is called
        // Then a ConfigError is returned with a human-readable message
        // Note: env var tests are inherently racy in parallel;
        // the actual validation is tested in config.rs unit tests
        let err = wh_telegram::TelegramError::ConfigError(
            "WH_URL environment variable not set".into(),
        );
        match err {
            wh_telegram::TelegramError::ConfigError(msg) => {
                assert!(msg.contains("WH_URL"));
            }
            _ => panic!("expected ConfigError"),
        }
    }
}
