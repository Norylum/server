use super::*;

#[tokio::test]
async fn adult_profile_requires_account_password() {
    const PASSWORD: &str = "another-secure-password-2026";

    let auth = AuthService::new(Database::in_memory().unwrap());

    auth.bootstrap_admin("admin", PASSWORD).await.unwrap();

    let session = auth.login("admin", PASSWORD).await.unwrap();

    let adult = auth
        .create_profile(&session.account_id, "Secondaire", false, None)
        .await
        .unwrap();

    assert!(matches!(
        auth.switch_profile(&session.access_token, &adult).await,
        Err(NorylumError::Forbidden(_))
    ));

    assert!(matches!(
        auth.switch_profile_with_password(
            &session.access_token,
            &adult,
            Some("mauvais-mot-de-passe"),
        )
        .await,
        Err(NorylumError::Forbidden(_))
    ));

    let allowed = auth
        .switch_profile_with_password(&session.access_token, &adult, Some(PASSWORD))
        .await
        .unwrap();

    let ctx = auth.authenticate(&allowed.access_token).await.unwrap();

    assert_eq!(ctx.profile_id(), &adult);
    assert!(!ctx.is_child());
}
