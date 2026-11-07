use leptos::prelude::*;

use crate::api::{get_token, list_shares};
use crate::components::{nav::Nav, status_badge::StatusBadge};

#[component]
pub fn Seeder() -> impl IntoView {
    if get_token().is_none() {
        let _ = web_sys::window()
            .and_then(|w| w.location().assign("/login").ok());
        return view! { <></> }.into_any();
    }

    let shares = Resource::new(|| (), |_| list_shares());

    view! {
        <div>
            <Nav/>
            <main class="p-8 space-y-4">
                <h1 class="text-2xl font-bold text-zinc-100">"Seeder"</h1>
                {move || match shares.get() {
                    None => view! { <p class="text-zinc-500">"Chargement…"</p> }.into_any(),
                    Some(Err(e)) => view! { <p class="text-red-400">{e}</p> }.into_any(),
                    Some(Ok(items)) if items.is_empty() => view! {
                        <p class="text-zinc-500 text-sm">"Aucun partage actif."</p>
                    }.into_any(),
                    Some(Ok(items)) => view! {
                        <div class="overflow-x-auto">
                            <table class="w-full text-sm text-left">
                                <thead class="text-xs text-zinc-500 uppercase border-b border-zinc-800">
                                    <tr>
                                        <th class="pb-2 pr-4">"Média"</th>
                                        <th class="pb-2 pr-4">"État"</th>
                                        <th class="pb-2 pr-4">"Pairs"</th>
                                        <th class="pb-2 pr-4">"Envoyé"</th>
                                        <th class="pb-2">"Débit"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {items.into_iter().map(|share| {
                                        let status = if share.enabled { "enabled" } else { "disabled" };
                                        let uploaded_mib = share.uploaded_bytes / (1024 * 1024);
                                        let upload_kbps = share.upload_bps / 1024;
                                        let peers = share.peer_count.map(|n| n.to_string()).unwrap_or_else(|| "—".into());
                                        view! {
                                            <tr class="border-b border-zinc-900 hover:bg-zinc-900/50">
                                                <td class="py-2 pr-4 font-mono text-xs text-zinc-400">{share.media_id}</td>
                                                <td class="py-2 pr-4">
                                                    <StatusBadge status=status.to_string()/>
                                                </td>
                                                <td class="py-2 pr-4 text-sm text-zinc-300">{peers}</td>
                                                <td class="py-2 pr-4 text-sm text-zinc-300">{format!("{uploaded_mib} MiB")}</td>
                                                <td class="py-2 text-sm text-zinc-300">{format!("{upload_kbps} KB/s")}</td>
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
