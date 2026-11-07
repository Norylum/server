use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::clear_token;

#[component]
pub fn Nav() -> impl IntoView {
    let logout = move |_| {
        clear_token();
        let _ = web_sys::window()
            .and_then(|w| w.location().assign("/login").ok());
    };

    view! {
        <nav class="bg-zinc-900 border-b border-zinc-800 px-6 py-3 flex items-center gap-6">
            <span class="font-bold text-zinc-100 mr-4">"Norylum"</span>
            <A href="/" class="text-zinc-400 hover:text-zinc-100 text-sm">"Dashboard"</A>
            <A href="/media" class="text-zinc-400 hover:text-zinc-100 text-sm">"Médiathèque"</A>
            <A href="/acquisitions" class="text-zinc-400 hover:text-zinc-100 text-sm">"Acquisitions"</A>
            <A href="/seeder" class="text-zinc-400 hover:text-zinc-100 text-sm">"Seeder"</A>
            <div class="ml-auto">
                <button
                    on:click=logout
                    class="text-xs text-zinc-500 hover:text-zinc-300"
                >
                    "Déconnexion"
                </button>
            </div>
        </nav>
    }
}
