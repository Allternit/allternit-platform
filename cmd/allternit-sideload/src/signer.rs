//! The sign flow over isideload's lower-level APIs (see bundle.rs for why not
//! `Sideloader::sign_app`). Talks to Apple and the anisette server; not unit
//! tested here (no network in tests). The pure pieces it uses are.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use isideload::anisette::remote_v3::RemoteV3AnisetteProvider;
use isideload::auth::apple_account::{AppleAccount, AppToken, TwoFactorCallbackParams, TwoFactorCallbackResponse};
use isideload::auth::builder::AppleAccountBuilder;
use isideload::dev::developer_session::{AppIdsApi, DeveloperSession, DeveloperTeam, DevicesApi, Profile, TeamsApi};
use isideload::sideload::application::Application;
use isideload::sideload::builder::MaxCertsBehavior;
use isideload::sideload::cert_identity::CertificateIdentity;
use isideload::sideload::sign;
use isideload::util::callbacks::MaxCertsCallbackBox;
use isideload::util::storage::SideloadingStorage;
use rootcause::prelude::*;
use tokio::sync::{mpsc, Mutex};

use crate::bundle::{self, PreparedApp};
use crate::bundle_ids::team_prefix;
use crate::errors::{classify_report, ErrorCode, Failure};
use crate::mac_anisette::MacAnisetteProvider;
use crate::protocol::{mask_phone, Done, Event, SignRequest, TwoFactorInput};
use crate::session::{self, StoredSession, SESSION_VERSION};
use crate::storage;

/// Shown on the certificate in the user's Apple developer account.
pub const MACHINE_NAME: &str = "Allternit Desktop";
/// How long we wait for the user to type a 2FA code.
pub const TWO_FACTOR_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

type TwoFactorFuture = Pin<Box<dyn Future<Output = Result<TwoFactorCallbackResponse, Report>> + Send>>;

pub struct Signer {
    pub anisette_url: String,
    pub emit: Emit,
    /// 2FA codes arriving on stdin.
    pub codes: Arc<Mutex<mpsc::Receiver<TwoFactorInput>>>,
    /// Builds a fresh storage handle (Keychain on macOS).
    pub storage: fn() -> Box<dyn SideloadingStorage>,
}

fn fail_report(report: Report, fallback: ErrorCode) -> Failure {
    classify_report(&report, fallback)
}

impl Signer {
    fn progress(&self, fraction: f32, step: &str) {
        (self.emit)(Event::progress(fraction, step));
    }

    async fn new_account(&self, apple_id: &str) -> Result<AppleAccount, Failure> {
        let builder = AppleAccountBuilder::new(apple_id);
        // This Mac's own AuthKit first (mac_anisette.rs says why); the server only when it gives nothing.
        let builder = match MacAnisetteProvider::probe() {
            Some(mac) => builder.anisette_provider(mac),
            None => builder.anisette_provider(
                RemoteV3AnisetteProvider::new(&self.anisette_url, (self.storage)(), "0".to_string())
                    .map_err(|r| fail_report(r, ErrorCode::AnisetteUnavailable))?,
            ),
        };
        builder
            .build()
            .await
            // Building fetches the client info (from /v3/client_info for the server): a failure here is anisette.
            .map_err(|r| fail_report(r, ErrorCode::AnisetteUnavailable))
    }

    fn two_factor_callback(&self) -> impl Fn(TwoFactorCallbackParams) -> TwoFactorFuture + Send + Sync {
        let emit = self.emit.clone();
        let codes = self.codes.clone();
        move |params: TwoFactorCallbackParams| -> TwoFactorFuture {
            let emit = emit.clone();
            let codes = codes.clone();
            Box::pin(async move {
                if params.unknown {
                    // Apple couldn't use the last method; fall back to a text
                    // to the first trusted number, else to trusted devices.
                    return Ok(match params.numbers.first() {
                        Some(n) if params.selected_number_id != Some(n.id) => TwoFactorCallbackResponse::SendSms(n.id),
                        _ => TwoFactorCallbackResponse::SendToDevices,
                    });
                }
                // On the device route this is the number a text can go to instead.
                let number = params
                    .selected_number_id
                    .and_then(|id| params.numbers.iter().find(|n| n.id == id))
                    .or(params.numbers.first());
                let phone = number.and_then(|n| mask_phone(&n.last_two_digits));
                emit(Event::TwoFactorRequired { sms: params.sms, phone, retry_reason: params.last_error.clone() });
                let mut rx = codes.lock().await;
                match tokio::time::timeout(TWO_FACTOR_TIMEOUT, rx.recv()).await {
                    Ok(Some(TwoFactorInput::Code(code))) => Ok(TwoFactorCallbackResponse::SubmitCode(code.expose().to_string())),
                    Ok(Some(TwoFactorInput::Text)) => Ok(match number {
                        Some(n) => TwoFactorCallbackResponse::SendSms(n.id),
                        // No trusted number on the account: ask the devices again.
                        None => TwoFactorCallbackResponse::ResendCode,
                    }),
                    // Timed out or stdin closed: isideload reports "No 2FA code provided".
                    _ => Ok(TwoFactorCallbackResponse::Abort),
                }
            })
        }
    }

    /// A developer session: saved one first, then a password login.
    async fn developer_session(
        &self,
        req: &SignRequest,
        store: &dyn SideloadingStorage,
    ) -> Result<(DeveloperSession, Option<StoredSession>, bool), Failure> {
        let apple_id = req.apple_id.trim();
        let saved = storage::load_session(store, apple_id);
        let mut account = self.new_account(apple_id).await?;

        if let Some(saved) = saved.as_ref() {
            self.progress(0.1, "Using your saved Apple sign-in");
            if let Some(dev) = self.resume(&mut account, saved, store, apple_id).await {
                return Ok(dev);
            }
        }

        let Some(password) = req.password() else {
            return Err(Failure::code(ErrorCode::PasswordRequired));
        };
        self.progress(0.12, "Signing in to Apple");
        account
            .login(password.expose(), self.two_factor_callback())
            .await
            .map_err(|r| fail_report(r, ErrorCode::BadCredentials))?;
        let token = account
            .get_app_token("xcode.auth")
            .await
            .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
        let adsid = adsid_of(&account)?;
        let stored = StoredSession {
            version: SESSION_VERSION,
            adsid: adsid.clone(),
            xcode_token: token.token.clone(),
            token_expiry_ms: session::expiry_to_ms(token.expiry),
            spd: encode_spd(&account),
            team_id: saved.as_ref().and_then(|s| s.team_id.clone()),
        };
        let dev = DeveloperSession::new(token, adsid, account.grandslam_client.clone(), account.anisette_generator.clone());
        Ok((dev, Some(stored), false))
    }

    /// Use a saved session; refresh its Xcode token from `spd` if it lapsed.
    /// None means "log in again".
    async fn resume(
        &self,
        account: &mut AppleAccount,
        saved: &StoredSession,
        store: &dyn SideloadingStorage,
        apple_id: &str,
    ) -> Option<(DeveloperSession, Option<StoredSession>, bool)> {
        let mut stored = saved.clone();
        let token = if saved.token_fresh(session::now_ms()) {
            AppToken { token: saved.xcode_token.clone(), duration: 0, expiry: saved.token_expiry_ms }
        } else {
            let spd = decode_spd(saved.spd.as_deref()?)?;
            account.spd = Some(spd);
            match account.get_app_token("xcode.auth").await {
                Ok(token) => {
                    stored.xcode_token = token.token.clone();
                    stored.token_expiry_ms = session::expiry_to_ms(token.expiry);
                    token
                }
                Err(_) => {
                    // The GrandSlam session itself is gone; only a password helps.
                    storage::forget_session(store, apple_id);
                    return None;
                }
            }
        };
        let mut dev = DeveloperSession::new(token, saved.adsid.clone(), account.grandslam_client.clone(), account.anisette_generator.clone());
        // Probe: a revoked token fails here rather than halfway through.
        match dev.list_teams().await {
            Ok(_) => Some((dev, Some(stored), true)),
            Err(_) => None,
        }
    }

    async fn pick_team(&self, dev: &mut DeveloperSession, preferred: Option<&str>) -> Result<DeveloperTeam, Failure> {
        let teams = dev.list_teams().await.map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
        let team = preferred
            .and_then(|id| teams.iter().find(|t| t.team_id == id))
            .or(teams.first())
            .cloned()
            .ok_or_else(|| Failure::new(ErrorCode::SigningFailed, "This Apple ID has no developer team. Sign in once at developer.apple.com to accept the agreement."))?;
        Ok(team)
    }

    pub async fn sign(&self, req: SignRequest) -> Result<Done, Failure> {
        std::fs::create_dir_all(&req.out_dir).map_err(|e| Failure::new(ErrorCode::SigningFailed, format!("Couldn't create the output folder: {e}")))?;
        for ipa in [&req.runner_ipa, &req.host_ipa] {
            if !ipa.is_file() {
                return Err(Failure::new(ErrorCode::InvalidRequest, format!("Missing package {}", ipa.display())));
            }
        }
        let store = (self.storage)();
        let apple_id = req.apple_id.trim().to_string();

        self.progress(0.05, "Connecting to Apple");
        let (mut dev, mut stored, session_reused) = self.developer_session(&req, store.as_ref()).await?;

        let preferred = stored.as_ref().and_then(|s| s.team_id.clone());
        let team = self.pick_team(&mut dev, preferred.as_deref()).await?;
        if let Some(s) = stored.as_mut() {
            s.team_id = Some(team.team_id.clone());
            // Saved as soon as it works, so a later failure (limits) doesn't cost the user another 2FA.
            let _ = storage::save_session(store.as_ref(), &apple_id, s);
        }
        let prefix = team_prefix(req.source_prefix(), &team.team_id)?;

        self.progress(0.3, "Registering this iPhone");
        dev.ensure_device_registered(&team, req.device_name.trim(), &req.udid, None)
            .await
            .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;

        self.progress(0.4, "Setting up your developer certificate");
        // Error, never Revoke: revoking would break the user's other sideloaded apps.
        let behavior: MaxCertsBehavior<MaxCertsCallbackBox> = MaxCertsBehavior::Error;
        // The signing key is stored per Apple ID; Apple IDs are case-insensitive.
        let key_owner = apple_id.to_lowercase();
        let cert = CertificateIdentity::retrieve(MACHINE_NAME, &key_owner, &mut dev, &team, store.as_ref(), &behavior)
            .await
            .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;

        self.progress(0.5, "Preparing the helper");
        let work = req.out_dir.join("sideload-work");
        let runner = prepare(&req.runner_ipa, &work.join("runner"), req.source_prefix(), &prefix)?;
        let host = prepare(&req.host_ipa, &work.join("host"), req.source_prefix(), &prefix)?;

        self.progress(0.6, "Registering the helper with Apple");
        // (bundle id, app name) for every App ID this sign needs: the two apps
        // and any real extensions. Never the .xctest (it shares the runner's).
        let mut all_ids: Vec<(String, String)> = Vec::new();
        for p in [&host, &runner] {
            all_ids.push((p.bundle_id.clone(), p.name.clone()));
            for id in &p.extension_ids {
                all_ids.push((id.clone(), p.name.clone()));
            }
        }
        let listed = dev.list_app_ids(&team, None).await.map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
        let existing: Vec<String> = listed.app_ids.iter().map(|a| a.identifier.clone()).collect();
        let needed: Vec<String> = all_ids.iter().map(|(id, _)| id.clone()).collect();
        for id in bundle::plan_app_ids(&needed, &existing, listed.available_quantity)? {
            let name = all_ids.iter().find(|(i, _)| *i == id).map(|(_, n)| n.as_str()).unwrap_or("Allternit");
            dev.add_app_id(&team, &format!("Allternit {name}"), &id, None)
                .await
                .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
        }
        let app_ids = dev.list_app_ids(&team, None).await.map_err(|r| fail_report(r, ErrorCode::SigningFailed))?.app_ids;

        let mut expiries: Vec<SystemTime> = Vec::new();
        let mut out: Vec<PathBuf> = Vec::new();
        for (i, (which, prepared)) in [("runner", &runner), ("host", &host)].into_iter().enumerate() {
            let base = 0.7 + 0.12 * i as f32;
            self.progress(base, if which == "runner" { "Signing the helper" } else { "Signing the helper app" });
            let profile_for = |id: &str| app_ids.iter().find(|a| a.identifier == id).cloned();
            let main_id = profile_for(&prepared.bundle_id)
                .ok_or_else(|| Failure::new(ErrorCode::SigningFailed, format!("Apple didn't list the App ID {}", prepared.bundle_id)))?;
            let main_profile = dev
                .download_team_provisioning_profile(&team, &main_id, None)
                .await
                .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
            expiries.push(SystemTime::from(main_profile.date_expire));

            let mut extra: Vec<(String, Profile, plist::Dictionary)> = Vec::new();
            for ext in &prepared.extension_ids {
                let app_id = profile_for(ext).ok_or_else(|| Failure::new(ErrorCode::SigningFailed, format!("Apple didn't list the App ID {ext}")))?;
                let profile = dev
                    .download_team_provisioning_profile(&team, &app_id, None)
                    .await
                    .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
                let entitlements = apple_codesign_quick::ProvisioningProfile::parse(profile.encoded_profile.as_ref())
                    .map_err(|e| Failure::new(ErrorCode::SigningFailed, format!("Couldn't read the profile for {ext}: {e}")))?
                    .entitlements()
                    .clone();
                expiries.push(SystemTime::from(profile.date_expire));
                extra.push((ext.clone(), profile, entitlements));
            }

            // The .xctest shares the runner's id and is in neither profile map,
            // so apple-codesign-quick signs it with the runner's identifier and
            // team, no entitlements and no embedded profile (= sign-ipa.sh).
            let mut app = Application::new(prepared.app_dir.clone()).map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;
            sign::sign(&mut app, &cert, &main_profile, &mut extra, &None, &team, None::<fn(f32) -> std::future::Ready<()>>)
                .await
                .map_err(|r| fail_report(r, ErrorCode::SigningFailed))?;

            let dest = req.out_dir.join(format!("{which}-signed.ipa"));
            bundle::zip_payload(&work.join(which), &dest)?;
            out.push(dest);
        }

        self.progress(0.97, "Cleaning up");
        let _ = std::fs::remove_dir_all(&work);
        if let Some(s) = stored.as_ref() {
            let _ = storage::save_session(store.as_ref(), &apple_id, s);
        }

        Ok(Done {
            runner_ipa: out[0].to_string_lossy().into_owned(),
            host_ipa: out[1].to_string_lossy().into_owned(),
            bundle_prefix: prefix,
            runner_bundle_id: runner.bundle_id.clone(),
            host_bundle_id: host.bundle_id.clone(),
            team_id: team.team_id.clone(),
            expires_at: expiries.into_iter().min().map(session::rfc3339),
            session_reused,
        })
    }
}

fn prepare(ipa: &Path, work: &Path, source_prefix: &str, target_prefix: &str) -> Result<PreparedApp, Failure> {
    let app = bundle::extract_ipa(ipa, work)?;
    bundle::rewrite_identities(&app, source_prefix, target_prefix)
}

fn adsid_of(account: &AppleAccount) -> Result<String, Failure> {
    account
        .spd
        .as_ref()
        .and_then(|spd| spd.get("adsid"))
        .and_then(plist::Value::as_string)
        .map(str::to_string)
        .ok_or_else(|| Failure::new(ErrorCode::SigningFailed, "Apple's sign-in response had no account id."))
}

fn encode_spd(account: &AppleAccount) -> Option<String> {
    let spd = account.spd.as_ref()?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    plist::to_writer_binary(&mut *bytes, &plist::Value::Dictionary(spd.clone())).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(&*bytes))
}

fn decode_spd(b64: &str) -> Option<plist::Dictionary> {
    let bytes = zeroize::Zeroizing::new(base64::engine::general_purpose::STANDARD.decode(b64).ok()?);
    plist::from_bytes::<plist::Dictionary>(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spd_round_trips_through_base64_plist() {
        let mut d = plist::Dictionary::new();
        d.insert("adsid".into(), plist::Value::String("000-1".into()));
        d.insert("sk".into(), plist::Value::Data(vec![1, 2, 3]));
        let b64 = {
            let mut bytes = Vec::new();
            plist::to_writer_binary(&mut bytes, &plist::Value::Dictionary(d.clone())).unwrap();
            base64::engine::general_purpose::STANDARD.encode(bytes)
        };
        assert_eq!(decode_spd(&b64), Some(d));
        assert_eq!(decode_spd("!!!"), None);
    }
}
