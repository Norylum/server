use leptos::prelude::*;

use crate::api::{get_token, list_media};
use crate::components::{nav::Nav, status_badge::StatusBadge};

#[component]
pub fn Media() -> impl IntoView {
    if get_token().is_none() {
        let _ = web_sys::window()
            .and_then(|w| w.location().assign("/login").ok());
        return view! { <></> }.into_any();
    }

    let media = Resource::new(|| (), |_| list_media());

    view! {
        <div>
            <Nav/>
            <main class="p-8 space-y-4">
                <h1 class="text-2xl font-bold text-zinc-100">"Médiathèque"</h1>
                {move || match media.get() {
                    None => view! { <p class="text-zinc-500">"Chargement…"</p> }.into_any(),
                    Some(Err(e)) => view! { <p class="text-red-400">{e}</p> }.into_any(),
                    Some(Ok(items)) => view! {
                        <div class="overflow-x-auto">
                            <table class="w-full text-sm text-left">
                                <thead class="text-xs text-zinc-500 uppercase border-b border-zinc-800">
                                    <tr>
                                        <th class="pb-2 pr-4">"Titre"</th>
                                        <th class="pb-2 pr-4">"Type"</th>
                                        <th class="pb-2">"ID"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {items.into_iter().map(|item| {
                                        let kind = item.media_type.clone().unwrap_or_default();
                                        view! {
                                            <tr class="border-b border-zinc-900 hover:bg-zinc-900/50">
                                                <td class="py-2 pr-4 text-zinc-100">{item.title}</td>
                                                <td class="py-2 pr-4">
                                                    <StatusBadge status=kind/>
                                                </td>
                                                <td class="py-2 text-zinc-500 font-mono text-xs">{item.id}</td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any(),
                }}
            </main>
        </div>
    }.into_any()
}
