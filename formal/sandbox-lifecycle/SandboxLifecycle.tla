-------------------------- MODULE SandboxLifecycle --------------------------
EXTENDS Naturals, TLC

CONSTANTS Workers, MaxClaims
ASSUME /\ Workers # {} /\ MaxClaims \in Nat \ {0}

VARIABLES db, workers, provider, claims, cancelled,
          lastCommitValid, lastCancelPreserved, lastErrorPreserved
vars == <<db, workers, provider, claims, cancelled,
          lastCommitValid, lastCancelPreserved, lastErrorPreserved>>

Idle == [phase |-> "idle", token |-> 0, operation |-> "inspect",
         result |-> "none"]

Init ==
    /\ db = [sandbox |-> "running", session |-> "ready", turn |-> "running",
              turnLeased |-> TRUE, operation |-> "inspect", complete |-> FALSE,
              token |-> 0, live |-> FALSE, runtime |-> "unknown", error |-> FALSE]
    /\ workers = [w \in Workers |-> Idle]
    /\ provider = "running"
    /\ claims = 0
    /\ cancelled = FALSE
    /\ lastCommitValid = TRUE
    /\ lastCancelPreserved = TRUE
    /\ lastErrorPreserved = TRUE

Valid(w) ==
    /\ db.token = workers[w].token
    /\ db.live
    /\ db.operation = workers[w].operation
    /\ ~db.complete

(* One committed transaction: session/turn fencing and durable cleanup intent.
   A rollback is a stuttering step. Repeated termination preserves its lease. *)
RequestTermination ==
    /\ db.sandbox # "ended"
    /\ db' = [db EXCEPT !.sandbox = "terminating", !.session = "ended",
               !.turn = "stopped", !.turnLeased = FALSE,
               !.operation = "terminate", !.complete = FALSE,
               !.token = IF db.operation = "inspect" \/ db.complete THEN 0
                         ELSE db.token,
               !.live = IF db.operation = "inspect" \/ db.complete THEN FALSE
                        ELSE db.live]
    /\ cancelled' = TRUE
    /\ lastCancelPreserved' =
         ((db.operation = "terminate" /\ ~db.complete /\ db.live)
          => (db'.token = db.token /\ db'.live))
    /\ UNCHANGED <<workers, provider, claims, lastCommitValid, lastErrorPreserved>>

(* The claim SQL is atomic. Unique bounded tokens stand in for fresh UUIDs.
   All pending work is considered due: retry timing affects latency, not safety. *)
Claim(w) ==
    /\ workers[w].phase = "idle"
    /\ ~db.complete /\ ~db.live
    /\ claims < MaxClaims
    /\ claims' = claims + 1
    /\ db' = [db EXCEPT !.token = claims + 1, !.live = TRUE]
    /\ workers' = [workers EXCEPT ![w] =
         [phase |-> "claimed", token |-> claims + 1,
          operation |-> db.operation, result |-> "none"]]
    /\ UNCHANGED <<provider, cancelled, lastCommitValid,
                   lastCancelPreserved, lastErrorPreserved>>

(* execute() validates the lease before external calls. Cancellation/expiry
   can still occur after this check and before the provider responds. *)
BeginCall(w) ==
    /\ workers[w].phase = "claimed"
    /\ Valid(w)
    /\ workers' = [workers EXCEPT ![w].phase = "calling"]
    /\ UNCHANGED <<db, provider, claims, cancelled, lastCommitValid,
                   lastCancelPreserved, lastErrorPreserved>>

ProviderReply(w, failed) ==
    /\ workers[w].phase = "calling"
    /\ workers' = [workers EXCEPT
         ![w].phase = "observed",
         ![w].result = IF failed THEN "error"
                      ELSE IF workers[w].operation = "terminate"
                           THEN "missing" ELSE provider]
    /\ provider' = IF ~failed /\ workers[w].operation = "terminate"
                    THEN "missing" ELSE provider
    /\ UNCHANGED <<db, claims, cancelled, lastCommitValid,
                   lastCancelPreserved, lastErrorPreserved>>

Expire ==
    /\ db.live
    /\ db' = [db EXCEPT !.live = FALSE]
    /\ UNCHANGED <<workers, provider, claims, cancelled, lastCommitValid,
                   lastCancelPreserved, lastErrorPreserved>>

(* A crash can lose a response after actual provider removal. No DB release is
   implied; the lease must expire before another claim. Also abstracts a fenced
   attempt returning without a result. *)
Abandon(w) ==
    /\ workers[w].phase # "idle"
    /\ workers' = [workers EXCEPT ![w] = Idle]
    /\ UNCHANGED <<db, provider, claims, cancelled, lastCommitValid,
                   lastCancelPreserved, lastErrorPreserved>>

(* One transaction after lock_sandbox() and locked lease validation. Provider
   effects and captured observations have already happened independently. *)
Finish(w) ==
    /\ workers[w].phase = "observed"
    /\ LET accepted == Valid(w)
           result == workers[w].result
           apply == accepted /\ db.sandbox # "ended" /\ result # "error"
           terminal == result = "missing"
       IN
       /\ db' = IF ~accepted THEN db ELSE
            [db EXCEPT
             !.sandbox = IF apply THEN IF terminal THEN "ended" ELSE result
                         ELSE db.sandbox,
             !.runtime = IF apply THEN result ELSE db.runtime,
             !.session = IF apply /\ result # "running" /\ db.session # "ended"
                         THEN "interrupted" ELSE db.session,
             !.turn = IF apply /\ result # "running" THEN "stopped" ELSE db.turn,
             !.turnLeased = IF apply /\ result # "running" THEN FALSE
                           ELSE db.turnLeased,
             !.complete = db.sandbox = "ended" \/ (apply /\ terminal),
             !.error = IF db.sandbox = "ended" THEN db.error ELSE result = "error",
             !.token = 0, !.live = FALSE]
       /\ lastCommitValid' = (~accepted \/ Valid(w))
       /\ lastErrorPreserved' =
            ((accepted /\ result = "error") =>
              (db'.sandbox = db.sandbox /\ db'.runtime = db.runtime /\
               db'.session = db.session /\ db'.turn = db.turn /\
               db'.turnLeased = db.turnLeased))
    /\ workers' = [workers EXCEPT ![w] = Idle]
    /\ UNCHANGED <<provider, claims, cancelled, lastCancelPreserved>>

(* Independent runtime changes; a missing resource is never recreated. *)
RuntimeChange(state) ==
    /\ provider # "missing"
    /\ state \in {"running", "stopped", "missing"}
    /\ provider' = state
    /\ UNCHANGED <<db, workers, claims, cancelled, lastCommitValid,
                   lastCancelPreserved, lastErrorPreserved>>

Next == RequestTermination \/ Expire
        \/ (\E state \in {"running", "stopped", "missing"}: RuntimeChange(state))
        \/ (\E w \in Workers:
              Claim(w) \/ BeginCall(w) \/ Abandon(w) \/ Finish(w)
              \/ (\E failed \in BOOLEAN: ProviderReply(w, failed)))

Spec == Init /\ [][Next]_vars

TypeOK ==
    /\ db \in [sandbox : {"running", "stopped", "terminating", "ended"},
               session : {"ready", "interrupted", "ended"},
               turn : {"running", "stopped"}, turnLeased : BOOLEAN,
               operation : {"inspect", "terminate"}, complete : BOOLEAN,
               token : 0..MaxClaims, live : BOOLEAN,
               runtime : {"unknown", "running", "stopped", "missing"},
               error : BOOLEAN]
    /\ workers \in [Workers ->
          [phase : {"idle", "claimed", "calling", "observed"},
           token : 0..MaxClaims, operation : {"inspect", "terminate"},
           result : {"none", "running", "stopped", "missing", "error"}]]
    /\ provider \in {"running", "stopped", "missing"}
    /\ claims \in 0..MaxClaims
    /\ <<cancelled, lastCommitValid, lastCancelPreserved, lastErrorPreserved>>
          \in BOOLEAN \X BOOLEAN \X BOOLEAN \X BOOLEAN

CancellationDurable ==
    cancelled =>
      /\ db.session = "ended" /\ db.turn = "stopped" /\ ~db.turnLeased
      /\ db.sandbox \in {"terminating", "ended"}
      /\ db.operation = "terminate"
      /\ (db.sandbox = "terminating" => ~db.complete)

TerminalSound == db.sandbox = "ended" =>
    (provider = "missing" /\ db.runtime = "missing" /\ db.complete)
WorkFenced == db.sandbox \in {"stopped", "terminating", "ended"} =>
    (db.turn = "stopped" /\ ~db.turnLeased)
LeaseSound == db.live => (db.token # 0 /\ ~db.complete)
OnlyCurrentLeaseCommits == lastCommitValid
RepeatedCancellationPreservesLease == lastCancelPreserved
ErrorsPreserveObservation == lastErrorPreserved
=============================================================================
