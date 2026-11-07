use leptos::prelude::*;

use crate::api::{get_token, list_media, list_shares};
use crate::components::nav::Nav;

#[component]
pub fn Dashboard() -> impl IntoView {
    if get_token().is_none() {
        let _ = web_sys::window()
            .and_then(|w| w.location().assign("/login").ok());
        return view! { <></> }.into_any();
    }

    let media_count = Resource::new(|| (), |_| list_media());
    let shares_count = Resource::new(|| (), |_| list_shares());

    view! {
        <div>
            <Nav/>
            <main class="p-8 space-y-6">
                <h1 class="text-2xl font-bold text-zinc-100">"Dashboard"</h1>
                <div class="grid grid-cols-1 sm:grid-cols-3 gap-4">
                    <div class="bg-zinc-900 border border-zinc-800 rounded-lg p-5">
                        <p class="text-xs text-zinc-500 mb-1">"Médias indexés"</p>
                        <p class="text-3xl font-bold text-zinc-100">
                            {move || match media_count.get() {
                                Some(Ok(v)) => v.len().to_string(),
                                Some(Err(_)) => "—".into(),
                                None => "…".into(),
                            }}
                        </p>
                    </div>
                    <div class="bg-zinc-900 border border-zinc-800 rounded-lg p-5">
                        <p class="text-xs text-zinc-500 mb-1">"Partages actifs"</p>
                        <p class="text-3xl font-bold text-zinc-100">
                            {move || match shares_count.get() {
                                Some(Ok(v)) => v.iter().filter(|s| s.enabled).count().to_string(),
                                Some(Err(_)) => "—".into(),
                                None => "…".into(),
                            }}
                        </p>
                    </div>
                </div>
            </main>
        </div>
    }.into_any()
}
