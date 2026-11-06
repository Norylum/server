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
    NORYLUM_VPN_COUNTRIES=Netherlands

Ne jamais publier les identifiants ni le fichier .env.

## Commandes

Controle local sans demarrer le VPN :

    python3 scripts/vpn_config.py
    docker compose --profile torrent config --quiet

Demarrer le tunnel sans seeding :

    scripts/norylum-docker.sh vpn-connect

Verifier la sortie reseau et le confinement :

    scripts/norylum-docker.sh vpn-status
    scripts/norylum-docker.sh vpn-egress-check
    scripts/norylum-docker.sh vpn-dns-check
    scripts/norylum-docker.sh vpn-firewall-check
    scripts/norylum-docker.sh vpn-ipv6-check

Arreter les services dependants du VPN :

    scripts/norylum-docker.sh vpn-stop

Apres validation complete du tunnel seulement :

    scripts/norylum-docker.sh vpn-start
    scripts/norylum-docker.sh torrent-status

vpn-start peut reprendre les partages deja actives en SQLite.
Verifier torrent-status avant cette commande.
torrent-enable demande une confirmation explicite.

## Diagnostic du confinement reseau

### vpn-egress-check

Compare l'IP publique de l'hote avec celle obtenue
depuis le conteneur vpn-probe.
Prouve : IP de sortie differente de celle de l'hote.
Ne prouve pas : absence de fuite DNS ou IPv6.

### vpn-dns-check

Affiche /etc/resolv.conf dans Gluetun et dans le
conteneur probe.
Un nameserver 127.0.0.1 signifie que le DNS passe
par le resolver DNS-over-TLS interne de Gluetun.
Ne prouve pas l'absence de fuite DNS : seule une
capture reseau sur tun0 peut l'exclure.

Commande de reference (dans le conteneur Gluetun) :

    docker exec <cid> cat /etc/resolv.conf

### vpn-firewall-check

Affiche les regles iptables et ip6tables OUTPUT
du conteneur Gluetun.
Prouve : politique DROP configuree statiquement.
Ne prouve pas : absence de contournement dynamique.

Commandes de reference :

    docker exec <cid> iptables -L OUTPUT -n -v
    docker exec <cid> ip6tables -L OUTPUT -n -v

### vpn-ipv6-check

Tente une connexion IPv6 sortante depuis vpn-probe.
Un echec confirme le blocage IPv6 au niveau firewall.
Ne prouve pas l'absence de fuite IPv6 par d'autres
mecanismes (tunnel 6to4, Teredo).

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

## Kill switch — validation 2026-11-06

Commande :

    scripts/norylum-docker.sh vpn-killswitch-test

Protocole execute :
Gluetun demarre dans un conteneur isole avec des
identifiants volontairement invalides (AUTH_FAILED).
Le VPN ne s'etablit pas. Les regles iptables sont
appliquees au demarrage, avant toute connexion VPN.

Resultats observes :
- Chain OUTPUT : policy DROP (confirme par iptables -L)
- Regles acceptees : loopback, RELATED/ESTABLISHED,
  subnet Docker, UDP vers le serveur OpenVPN (port 1194),
  trafic via tun0 (interface VPN, non etablie).
- Connexion IPv4 sortante : bloquee.
- Connexion IPv6 sortante : bloquee.

Conclusion : comportement fail-closed confirme.
Le pare-feu Gluetun bloque tout trafic non-VPN
meme lorsque la connexion VPN ne s'est pas etablie.

Pour relancer le test :

    scripts/norylum-docker.sh vpn-killswitch-test

## Kill switch — tunnel actif — validation 2026-11-06

Commande :

    scripts/norylum-docker.sh vpn-killswitch-active-test

Protocole execute :
Gluetun demarre dans un conteneur isole avec les
credentials NordVPN reels (montage vpn-secrets en
lecture seule). Le test attend healthy, verifie la
connectivite, envoie SIGKILL au processus OpenVPN
(PID 50), puis lance immediatement un probe IPv4.

Resultats observes :
- Phase 1 : connectivite IPv4 etablie via VPN
  (IP de sortie : 186.247.77.219).
- Phase 2 : connexion IPv4 bloquee immediatement
  apres SIGKILL sur OpenVPN.

Conclusion : kill switch operationnel sur tunnel actif.
Le trafic est bloque sans delai apres la coupure.

## Limitations NordVPN

NordVPN standard ne propose pas de redirection
de port entrante. Certains pairs ne pourront donc
pas se connecter directement au seeder.

Le seeder sera visible uniquement des pairs qui
initient la connexion. Le swarm de partage sera
reduit par rapport a un seeder avec port forwarding.

Certains serveurs NordVPN peuvent etre satures ou
bloques selon le contenu. Changer de pays ou de
serveur peut ameliorer la connectivite.

## Procedure de retour arriere

Pour revenir a un etat sans VPN ni seeder :

    scripts/norylum-docker.sh vpn-stop

Le serveur Norylum continue a fonctionner.
La base SQLite n'est pas affectee.

Pour retablir le VPN :

    scripts/norylum-docker.sh vpn-connect

Pour retablir le VPN et le seeder :

    scripts/norylum-docker.sh vpn-start

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
Kill switch valide : fail-closed et coupure tunnel actif (2026-11-06).
Acquisition BitTorrent automatique non implementee.
Ne pas annoncer le parcours complet comme termine.


## Validation NordVPN — 2026-11-03

Configuration testee :
- Gluetun v3.41.3
- NordVPN OpenVPN UDP
- Pays-Bas
- Liste de serveurs actualisee dans vpn-state/

Resultats observes :
- Tunnel initialise et Gluetun healthy.
- IPv4 publique VPN distincte de celle du Mac.
- HTTPS via VPN fonctionnel.
- Connexion IPv6 externe non etablie.
- Pare-feu IPv4 et IPv6 en politique OUTPUT DROP.
- HTTPS par tun0 : HTTP 301.
- HTTPS par eth0 : timeout.
- Seeder arrete et API Norylum healthy.

## Validation diagnostics live — 2026-11-06

Commandes executees sur le tunnel de production actif.

vpn-dns-check :
- Gluetun et probe : nameserver 127.0.0.1 (DNS-over-TLS interne).
- Aucun resolver externe visible dans resolv.conf.

vpn-firewall-check :
- IPv4 Chain OUTPUT policy DROP confirme.
  Regles actives : loopback, RELATED/ESTABLISHED, subnet Docker,
  UDP vers serveur NordVPN (91.132.138.211:1194), tun0.
  15 paquets rejetes par la politique DROP.
- IPv6 Chain OUTPUT policy DROP confirme.
  Regles : loopback, RELATED/ESTABLISHED, multicast NDP, tun0.
  0 paquet IPv6 rejete (aucune tentative de sortie IPv6 observee).

vpn-ipv6-check :
- Connexion IPv6 sortante bloquee (curl --ipv6 : echec attendu).

vpn-egress-check :
- IP de sortie VPN distincte de celle de l'hote.

Aucune certification anti-fuite complete n'est revendiquee :
le trafic DNS n'a pas ete capture par tcpdump.

## Risques residuels

| Risque | Statut |
|---|---|
| Fuite DNS par requete hors tunnel | Non exclue — DNS resolver 127.0.0.1 verifie structurellement, trafic non capture |
| Contournement IPv6 (Teredo, 6to4) | Non exclu — firewall DROP verifie, mecanismes de tunnel non testes |
| Kill switch au demarrage sans VPN | **Teste et confirme** (2026-11-06) |
| Kill switch apres coupure tunnel actif | **Teste et confirme** (2026-11-06) |
| Logs VPN persistes sur hote | Limite par rotation (10 Mo, 3 fichiers) |
| Seeder actif si Gluetun redémarre | Non possible — restart:no, depends_on:service_healthy |
| Port BitTorrent expose sur hote | Absent — aucun port publie dans compose.yaml |
| Seeder demarrant sans VPN verifie | Bloque — vpn_egress_check requis avant up seeder |
