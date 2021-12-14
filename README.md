# aws-sso-env

A Rust utility for extracting SSO credentials and exporting them into your shell environment.

## Building and Installation

Install [rustup][rustup] and a working Rust compiler, then use `cargo build` to build the project. To install, run
`cargo install --path .` to compile a release binary and install it in `~/.cargo/bin`.

## Configuration

If you haven't setup SSO yet on your machine, you will need to use the AWS CLI to log in. This can be accomplished via
`aws --profile profile-name sso login`, after you have installed an SSO profile in `~/.aws/config`:

```ini
[profile prod]
sso_start_url = https://${MY_ACCOUNT_ID}.awsapps.com/start
sso_region = $SSO_REGION
sso_account_id = $MY_ACCOUNT_ID
sso_role_name = $SSO_ROLE_NAME
region = $REGION
```

After the initial login, a cached credential token will exist in `~/.aws/sso/cache`. This token will only be valid for
a certain period of time, after which, you'll need to call `aws --profile profile-name sso login` to renew the token.
This utility will not initiate the browser-based login for you, whereas the AWS CLI _will_ initiate the browser-based
login.

## Usage

```text
USAGE:
    aws-sso-env <profile-name>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

ARGS:
    <profile-name>    The name of an SSO profile in your local AWS configuration file(s)
```

To produce credentials for a profile named `prod`:

```shell
aws-sso-env prod
```

This will produce lines starting with `export` for the `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and
`AWS_SESSION_TOKEN` variables. Copy and paste these lines into your shell session to set these variables, or even
better: export them directly into your shell, without leaving credentials in your shell history:

```shell
$(aws-sso-env prod)
```

## License

Licensed at your discretion under either

 - [Apache Software License, Version 2.0](./LICENSE-APACHE)
 - [MIT License](./LICENSE-MIT)

 [rustup]: https://rustup.rs