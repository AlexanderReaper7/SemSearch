open Printf

(* First. *)
let one x =
  let y = x + 1 in
  y * 2

(** Second. *)
let two items =
  List.fold_left
    (fun total item -> total + item)
    0 items
