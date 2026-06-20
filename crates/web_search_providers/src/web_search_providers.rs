mod cloud;

use client::{Client, UserStore};
use gpui::{App, Context, Entity};
use language_model::LanguageModelRegistry;
use std::sync::Arc;
use web_search::{WebSearchProviderId, WebSearchRegistry};

/// Auracle white-label: the hosted Zed-cloud web-search provider is the only
/// web-search backend, and it is gated on the hosted cloud LLM provider being
/// the default model. Since Auracle does not register that hosted provider, web
/// search is disabled. This flag keeps the registration off explicitly; the
/// `web_search` tool degrades gracefully ("Web search is not available.") when
/// no provider is active.
const ENABLE_HOSTED_WEB_SEARCH: bool = false;

pub fn init(client: Arc<Client>, user_store: Entity<UserStore>, cx: &mut App) {
    let registry = WebSearchRegistry::global(cx);
    registry.update(cx, |registry, cx| {
        register_web_search_providers(registry, client, user_store, cx);
    });
}

fn register_web_search_providers(
    registry: &mut WebSearchRegistry,
    client: Arc<Client>,
    user_store: Entity<UserStore>,
    cx: &mut Context<WebSearchRegistry>,
) {
    if !ENABLE_HOSTED_WEB_SEARCH {
        return;
    }

    register_zed_web_search_provider(
        registry,
        client.clone(),
        user_store.clone(),
        &LanguageModelRegistry::global(cx),
        cx,
    );

    cx.subscribe(
        &LanguageModelRegistry::global(cx),
        move |this, registry, event, cx| {
            if let language_model::Event::DefaultModelChanged = event {
                register_zed_web_search_provider(
                    this,
                    client.clone(),
                    user_store.clone(),
                    &registry,
                    cx,
                )
            }
        },
    )
    .detach();
}

fn register_zed_web_search_provider(
    registry: &mut WebSearchRegistry,
    client: Arc<Client>,
    user_store: Entity<UserStore>,
    language_model_registry: &Entity<LanguageModelRegistry>,
    cx: &mut Context<WebSearchRegistry>,
) {
    let using_zed_provider = language_model_registry
        .read(cx)
        .default_model()
        .is_some_and(|default| default.is_provided_by_zed());
    if using_zed_provider {
        registry.register_provider(
            cloud::CloudWebSearchProvider::new(client, user_store, cx),
            cx,
        )
    } else {
        registry.unregister_provider(WebSearchProviderId(
            cloud::ZED_WEB_SEARCH_PROVIDER_ID.into(),
        ));
    }
}
