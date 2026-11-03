# Norylum — NordVPN et BitTorrent

## Principes

Le serveur Norylum fonctionne sans VPN ni seeder.
Le VPN et BitTorrent sont facultatifs et desactives
par defaut.

Le seeder Rust utilise librqbit avec un volume de
snapshots distinct des fichiers originaux.

Un partage exige une autorisation explicite et les
droits de distribution du media concerne.

## NordVPN / Gluetun

Configuration conseillee pour le premier essai :
NordVPN avec OpenVPN.

Le compte NordVPN fournit des identifiants de service
distincts du mot de passe habituel Nord Account.

Fichiers locaux, non suivis par Git :

- vpn-secrets/openvpn_user
- vpn-secrets/openvpn_password

Permissions : repertoire 700, fichiers 600.

Configuration locale .env :

    NORYLUM_VPN_PROVIDER=nordvpn
    NORYLUM_VPN_TYPE=openvpn
    NORYLUM_VPN_COUNTRIES=France

Ne jamais publier les identifiants ni le fichier .env.

## Commandes

Controle local sans demarrer le VPN :

    python3 scripts/vpn_config.py
    docker compose --profile torrent config --quiet

Demarrer le tunnel sans seeding :

    scripts/norylum-docker.sh vpn-connect

Verifier la sortie reseau :

    scripts/norylum-docker.sh vpn-status
    scripts/norylum-docker.sh vpn-egress-check

Arreter les services dependants du VPN :

    scripts/norylum-docker.sh vpn-stop

Apres validation complete du tunnel seulement :

    scripts/norylum-docker.sh vpn-start
    scripts/norylum-docker.sh torrent-status

vpn-start peut reprendre les partages deja actives en SQLite.
Verifier torrent-status avant cette commande.
torrent-enable demande une confirmation explicite.

## Validations de securite obligatoires

- Verifier les IP de sortie et les routes.
- Tester les fuites DNS et IPv6.
- Tester le kill switch pendant une coupure controlee.
- Verifier l'absence de reseau Docker alternatif.
- Refuser le seeding si Gluetun n'est pas healthy.
- Confirmer qu'aucun port torrent n'est expose sur l'hote.
- Verifier l'arret et la reconnexion apres interruption.

Le statut healthy et une IP differente de l'hote
ne constituent pas une certification anti-fuite.

NordVPN standard ne propose pas de redirection
de port entrante. Certains pairs ne pourront donc
pas se connecter directement au seeder.

## Acquisition future

Les fournisseurs de recherche seront interchangeables.
TMDB servira a identifier l'oeuvre et a construire
les requetes de recherche, pas a calculer un infohash.

Les resultats des indexeurs seront des candidats,
pas des preuves d'identite ou de droits sur un film.

Les magnets necessiteront une validation explicite.
Aucun indexeur, magnet ni telechargement BitTorrent
automatique n'est encore raccorde au worker.

## Streaming et interface

Le Direct Play reste prioritaire.
Le transcodage sera active seulement si necessaire.

Backends envisages :
QSV, NVENC, VAAPI, VideoToolbox et repli logiciel.
L'acceleration depend du GPU, des pilotes et du
contexte natif ou Docker.

Le frontend Web sera facultatif, dans un conteneur
separe. Le serveur headless gardera toute son API.

## Etat actuel

Infrastructure de seeding implementee.
NordVPN reel, DNS, IPv6 et kill switch non valides.
Acquisition BitTorrent automatique non implementee.
Ne pas annoncer le parcours complet comme termine.
