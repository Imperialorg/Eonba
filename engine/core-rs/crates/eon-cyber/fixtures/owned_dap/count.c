/* Deliberately different callers make a conditional hit observable from the
 * source stack alone. No arguments, I/O, allocation, or host data. */
static int count(int visit) {
    return visit + 1;
}

static int first(void) {
    return count(1);
}

static int second(void) {
    return count(2);
}

static int third(void) {
    return count(3);
}

int main(void) {
    int a = first();
    int b = second();
    int c = third();
    return a + b + c == 9 ? 0 : 1;
}
