use leptos::prelude::*;

#[component]
pub fn StatusBadge(status: String) -> impl IntoView {
    let class = match status.as_str() {
        "available" | "running" | "enabled" => "bg-green-900 text-green-300",
        "pending" | "approved" => "bg-yellow-900 text-yellow-300",
        "failed" | "unavailable" | "cancelled" => "bg-red-900 text-red-300",
        _ => "bg-zinc-800 text-zinc-400",
    };
    view! {
        <span class=format!("px-2 py-0.5 rounded text-xs font-mono {class}")>
            {status}
        </span>
    }
}
