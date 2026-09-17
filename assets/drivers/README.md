# Driver marks

The three brand logos shown wherever a connection says which database it speaks to.

| file | project | trademark | source |
|---|---|---|---|
| `trino.svg` | [Trino](https://trino.io) | Trino Software Foundation | [simple-icons](https://github.com/simple-icons/simple-icons) (CC0-1.0) |
| `postgresql.svg` | [PostgreSQL](https://www.postgresql.org) | PostgreSQL Community Association | [simple-icons](https://github.com/simple-icons/simple-icons) (CC0-1.0) |
| `mysql.svg` | [MySQL](https://www.mysql.com) | Oracle Corporation | [devicon](https://github.com/devicons/devicon) (MIT) |

MySQL comes from a different set on purpose: simple-icons ships the mark *with* its wordmark,
which sat unevenly beside two symbol-only logos in the type grid. devicon's is the dolphin alone.

The sets are permissively licensed, but that covers the *files*, not the brands. The marks remain
the trademarks of the projects above and are used here for the one purpose trademark law has
always allowed — saying *which* product a connection talks to — not to suggest that any of those
projects endorses this tool.

Replacing a file and running `app/make-driver-logos.sh` is the whole update procedure: the Swift
that draws them is generated from these, never edited.
