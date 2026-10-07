//! Prints the Platform API's Stripe plan (a dry run). See
//! `routes::platform_v1::stripe_plan`: `--json` for JSON, `--apply` creates the
//! objects only when `ALLTERNIT_PLATFORM_STRIPE_APPLY=1` and `STRIPE_SECRET_KEY` are set.

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(allternit_cloud_api::routes::platform_v1::stripe_plan::cli_main(&args).await);
}
