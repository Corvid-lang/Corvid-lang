create table users (
  id text primary key,
  email text not null,
  display_name text not null
);

create table tasks (
  id text primary key,
  user_id text not null,
  title text not null,
  status text not null
);

create table approvals (
  id text primary key,
  actor_id text not null,
  action text not null,
  subject text not null,
  state text not null,
  trace_id text not null
);

create table traces (
  id text primary key,
  replay_key text not null,
  summary text not null
);

create table connector_tokens (
  id text primary key,
  provider text not null,
  account_id text not null,
  ciphertext_hash text not null,
  key_id text not null
);

create table agent_state (
  id text primary key,
  user_id text not null,
  agent_name text not null,
  checkpoint text not null,
  replay_key text not null
);
