# The pass drop zone: OpenSSH doing SFTP into one directory. See README.md.
#   docker build -t pass-dropzone .
#   docker run -d -p 2222:2222 -v pass-dropzone:/data -e DROPZONE_KEY_PC='ssh-ed25519 AAAA...' \
#       -e DROPZONE_KEY_PHONE='ssh-ed25519 AAAA...' pass-dropzone
FROM alpine:3.22

# The account has no password and no shell. `*` and not `!`: sshd treats `!` as a locked account
# and refuses its keys too.
RUN apk add --no-cache openssh-server openssh-keygen \
    && adduser -D -H -h /drop -s /sbin/nologin drop \
    && sed -i 's/^drop:!/drop:*/' /etc/shadow \
    && rm -f /etc/ssh/ssh_host_*

COPY entrypoint.sh /entrypoint.sh

VOLUME /data
EXPOSE 2222
ENTRYPOINT ["/entrypoint.sh"]
